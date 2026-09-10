//! Everything a guest can draw into or observe during a frame: the buffer
//! slab with the screen in it, the pen, the palette, the frame log and
//! the cart it may load assets from. The guest borrows it for a step and
//! holds no pixels of its own.

use std::rc::Rc;

use crate::assets;
use crate::audio;
use crate::blit;
use crate::buf::{BufKind, MapInfo, Rect};
use crate::manifest::valid_asset_name;
use crate::meter::FrameProfile;
use crate::palette::{Palette, Rgb};
use crate::pen::{Colour, Fillp, Pen};
use crate::raster::{self, Surface};
use crate::resources::{BufId, GfxError, Resources};
use crate::save::{MemoryStore, SaveStore};
use crate::snapshot::Snapshot;
use crate::source::CartSource;

/// Most log lines kept per frame; later ones are dropped with a marker.
pub const MAX_LOG_LINES: usize = 256;
/// Longest log line kept; longer ones are cut.
pub const MAX_LOG_LINE_BYTES: usize = 1024;

pub struct DrawState {
    pub res: Resources,
    pub pen: Pen,
    pub palette: Palette,
    /// Lines the cart logged this frame. Cleared by the console before
    /// each step.
    pub log: Vec<String>,
    pub cart: Rc<dyn CartSource>,
    /// What the last step cost, written by the guest at the end of its
    /// step. A guest without a meter leaves it zero.
    pub profile: FrameProfile,
    /// The mixer; rendered by the console after each step.
    pub audio: audio::Mixer,
    /// The cart's save slots. In memory unless the host
    /// installs a file store through `Console::set_save_store`.
    pub saves: Box<dyn SaveStore>,
    /// The data behind `sys`, present only in the shell's state
    ///. A cart's state has none.
    pub sys: Option<crate::shell::SysState>,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for DrawState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrawState")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("live_bufs", &self.res.live_count())
            .field("log", &self.log)
            .finish()
    }
}

impl Default for DrawState {
    fn default() -> DrawState {
        DrawState::new(320, 240, Rc::new(Snapshot::empty()))
    }
}

impl DrawState {
    pub fn new(width: u32, height: u32, cart: Rc<dyn CartSource>) -> DrawState {
        DrawState {
            res: Resources::new(width, height),
            pen: Pen::new(BufId::SCREEN, width, height),
            palette: Palette::default(),
            log: Vec::new(),
            cart,
            profile: FrameProfile::default(),
            audio: audio::Mixer::new(),
            saves: Box::new(MemoryStore::new()),
            sys: None,
            width,
            height,
        }
    }

    /// A zero-size placeholder for swapping a real state out of a slot.
    pub fn placeholder() -> DrawState {
        DrawState::new(0, 0, Rc::new(Snapshot::empty()))
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn screen_pixels(&self) -> &[u8] {
        self.res.screen().as_u8().expect("screen is u8")
    }

    /// The screen's bytes, writable: for a guest that receives its frames
    /// from another process rather than drawing them here.
    pub fn screen_pixels_mut(&mut self) -> &mut [u8] {
        self.res.screen_mut().as_u8_mut().expect("screen is u8")
    }

    /// Append a log line within the per-frame bounds.
    pub fn log_line(&mut self, mut line: String) {
        if self.log.len() >= MAX_LOG_LINES {
            if self.log.len() == MAX_LOG_LINES {
                self.log
                    .push(format!("... log truncated at {MAX_LOG_LINES} lines"));
            }
            return;
        }
        if line.len() > MAX_LOG_LINE_BYTES {
            let mut cut = MAX_LOG_LINE_BYTES;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
            line.push_str("...");
        }
        self.log.push(line);
    }

    // ----- target and pen -------------------------------------------------

    fn target(&mut self) -> (Surface<'_>, &Pen) {
        if !self.res.is_live(self.pen.target) {
            self.retarget_screen();
        }
        let buf = self.res.get_mut(self.pen.target).expect("target is live");
        (Surface::of(buf).expect("target is u8"), &self.pen)
    }

    fn retarget_screen(&mut self) {
        self.pen.target = BufId::SCREEN;
        self.pen.clip = Rect::new(0, 0, self.width as i32, self.height as i32);
    }

    /// Retarget to a `u8` buffer, or the screen for `None`. The clip
    /// resets to the new target.
    pub fn draw_target(&mut self, id: Option<BufId>) -> Result<(), GfxError> {
        let id = id.unwrap_or(BufId::SCREEN);
        let buf = self.res.get(id)?;
        if buf.kind() != BufKind::U8 {
            return Err(GfxError::KindMismatch {
                expected: BufKind::U8,
                got: buf.kind(),
            });
        }
        let (w, h) = (buf.width() as i32, buf.height() as i32);
        self.pen.target = id;
        self.pen.clip = Rect::new(0, 0, w, h);
        Ok(())
    }

    /// Clip in target coordinates; `None` resets to the whole target.
    pub fn clip(&mut self, rect: Option<Rect>) {
        let buf = match self.res.get(self.pen.target) {
            Ok(b) => b,
            Err(_) => {
                self.retarget_screen();
                self.res.screen()
            }
        };
        let full = buf.bounds();
        self.pen.clip = match rect {
            Some(r) => r.intersect(&full),
            None => full,
        };
    }

    pub fn camera(&mut self, x: i32, y: i32) {
        self.pen.camera = (x, y);
    }

    pub fn fillp(&mut self, f: Fillp) {
        self.pen.fillp = f;
    }

    // ----- palette --------------------------------------------------------

    pub fn pal(&mut self, index: usize, rgb: Rgb) -> Result<(), GfxError> {
        Ok(self.palette.set(index, rgb)?)
    }

    pub fn pal_restore(&mut self) {
        self.palette = Palette::default();
    }

    pub fn palt(&mut self, i: u8, transparent: bool) {
        self.pen.table.palt(i, transparent);
    }

    pub fn palt_clear(&mut self) {
        self.pen.table.clear_transparency();
    }

    pub fn pal_map(&mut self, from: u8, to: u8) {
        self.pen.table.pal_map(from, to);
    }

    pub fn pal_map_clear(&mut self) {
        self.pen.table.clear_remap();
    }

    pub fn pal_reset(&mut self) {
        self.pen.table.reset();
    }

    // ----- primitives -----------------------------------------------------

    // Every primitive returns the pixels it touched after clipping, which
    // is what the cycle meter prices.

    pub fn cls(&mut self, c: u8) -> u64 {
        let (mut s, pen) = self.target();
        raster::cls(&mut s, pen, c);
        s.touched
    }

    pub fn pset(&mut self, x: i32, y: i32, c: Colour) -> u64 {
        let (mut s, pen) = self.target();
        raster::pset(&mut s, pen, x, y, c);
        s.touched
    }

    pub fn pget(&mut self, x: i32, y: i32) -> u8 {
        let (s, _) = self.target();
        raster::pget(&s, x, y)
    }

    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Colour) -> u64 {
        let (mut s, pen) = self.target();
        raster::line(&mut s, pen, x0, y0, x1, y1, c);
        s.touched
    }

    pub fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Colour) -> u64 {
        let (mut s, pen) = self.target();
        raster::rect(&mut s, pen, x0, y0, x1, y1, c);
        s.touched
    }

    pub fn rectfill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Colour) -> u64 {
        let (mut s, pen) = self.target();
        raster::rectfill(&mut s, pen, x0, y0, x1, y1, c);
        s.touched
    }

    /// The midpoint walk is not clipped, so the work is proportional to
    /// the radius even when almost no pixel lands in the clip: the count
    /// returned is at least the perimeter.
    pub fn circ(&mut self, x: i32, y: i32, r: i32, c: Colour) -> u64 {
        let (mut s, pen) = self.target();
        raster::circ(&mut s, pen, x, y, r, c);
        s.touched.max(raster::circle_work(r))
    }

    /// Like [`DrawState::circ`]: the half-width table costs the radius
    /// whatever the clip.
    pub fn circfill(&mut self, x: i32, y: i32, r: i32, c: Colour) -> u64 {
        let (mut s, pen) = self.target();
        raster::circfill(&mut s, pen, x, y, r, c);
        s.touched.max(raster::circle_work(r))
    }

    /// Returns the x after the last glyph and the pixels touched.
    pub fn print(&mut self, text: &str, x: i32, y: i32, c: u8) -> (i32, u64) {
        let (mut s, pen) = self.target();
        let end = blit::print(&mut s, pen, text, x, y, c);
        (end, s.touched)
    }

    // ----- sheets and maps ------------------------------------------------

    /// Select the sheet `spr`, `sspr` and `map` read from.
    pub fn sheet(&mut self, id: Option<BufId>) -> Result<(), GfxError> {
        if let Some(id) = id {
            let buf = self.res.get(id)?;
            if buf.kind() != BufKind::U8 {
                return Err(GfxError::KindMismatch {
                    expected: BufKind::U8,
                    got: buf.kind(),
                });
            }
        }
        self.pen.sheet = id;
        Ok(())
    }

    fn sheet_and_target(&mut self) -> Result<(Surface<'_>, &crate::buf::Buf, &Pen), GfxError> {
        let sheet = self.pen.sheet.ok_or(GfxError::NoSheet)?;
        if !self.res.is_live(self.pen.target) {
            self.retarget_screen();
        }
        self.res.get(sheet)?;
        let (target, sheet) = self.res.pair_mut(self.pen.target, sheet)?;
        Ok((Surface::of(target).expect("target is u8"), sheet, &self.pen))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spr(
        &mut self,
        n: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        flip_x: bool,
        flip_y: bool,
    ) -> Result<u64, GfxError> {
        let (mut s, sheet, pen) = self.sheet_and_target()?;
        blit::spr(&mut s, pen, sheet, n, x, y, w, h, flip_x, flip_y);
        Ok(s.touched)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sspr(
        &mut self,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        dx: i32,
        dy: i32,
        dw: i32,
        dh: i32,
        flip_x: bool,
        flip_y: bool,
    ) -> Result<u64, GfxError> {
        let (mut s, sheet, pen) = self.sheet_and_target()?;
        blit::sspr(
            &mut s, pen, sheet, sx, sy, sw, sh, dx, dy, dw, dh, flip_x, flip_y,
        );
        Ok(s.touched)
    }

    /// Draw one layer of a map through the current sheet.
    #[allow(clippy::too_many_arguments)]
    pub fn map(
        &mut self,
        map: BufId,
        cell_x: i32,
        cell_y: i32,
        sx: i32,
        sy: i32,
        cell_w: i32,
        cell_h: i32,
        layer: i32,
    ) -> Result<u64, GfxError> {
        let (cells, info, map_w) = {
            let buf = self.res.get(map)?;
            let info = buf.map_info().ok_or(GfxError::NotAMap)?;
            if layer < 0 || layer as u32 >= info.layers {
                return Ok(0);
            }
            let per_layer = buf.width() as usize * info.rows as usize;
            let start = layer as usize * per_layer;
            let cells = buf.as_i16().ok_or(GfxError::NotAMap)?[start..start + per_layer].to_vec();
            (cells, info, buf.width() as i32)
        };
        let (mut s, sheet, pen) = self.sheet_and_target()?;
        blit::map(
            &mut s,
            pen,
            sheet,
            &cells,
            map_w,
            info.rows as i32,
            info.tile_size as i32,
            cell_x,
            cell_y,
            sx,
            sy,
            cell_w,
            cell_h,
        );
        Ok(s.touched)
    }

    /// Decode `gfx/<name>.png`, or return the live buffer of that name.
    pub fn load_sheet(&mut self, name: &str) -> Result<BufId, GfxError> {
        let path = assets::sheet_path(name);
        if !valid_asset_name(name) {
            return Err(GfxError::asset_invalid(&path, "bad asset name"));
        }
        if let Some(id) = self.res.named(&path) {
            return Ok(id);
        }
        let bytes = self
            .cart
            .read(&path)
            .map_err(|_| GfxError::AssetNotFound { path: path.clone() })?;
        let id = assets::decode_sheet(&path, &bytes, &mut self.res)?;
        self.res.set_name(&path, id);
        Ok(id)
    }

    /// Decode `map/<name>.json`, or return the live buffer of that name.
    pub fn load_map(&mut self, name: &str) -> Result<BufId, GfxError> {
        let path = assets::map_path(name);
        if !valid_asset_name(name) {
            return Err(GfxError::asset_invalid(&path, "bad asset name"));
        }
        if let Some(id) = self.res.named(&path) {
            return Ok(id);
        }
        let bytes = self
            .cart
            .read(&path)
            .map_err(|_| GfxError::AssetNotFound { path: path.clone() })?;
        let id = assets::decode_map(&path, &bytes, &mut self.res)?;
        self.res.set_name(&path, id);
        Ok(id)
    }

    // ----- buffers --------------------------------------------------------

    pub fn buf_alloc(&mut self, kind: BufKind, w: u32, h: u32) -> Result<BufId, GfxError> {
        self.res.alloc(kind, w, h)
    }

    /// Explicit release. Releasing the current target or sheet clears it.
    pub fn buf_release(&mut self, id: BufId) -> Result<(), GfxError> {
        self.res.free(id)?;
        if self.pen.target == id {
            self.retarget_screen();
        }
        if self.pen.sheet == Some(id) {
            self.pen.sheet = None;
        }
        Ok(())
    }

    /// Free whatever the guest's collector has let go of.
    pub fn reap(&mut self, ids: impl IntoIterator<Item = BufId>) {
        for id in ids {
            if self.res.is_live(id) {
                let _ = self.buf_release(id);
            }
        }
    }

    pub fn buf_info(&self, id: BufId) -> Result<(BufKind, u32, u32, Option<MapInfo>), GfxError> {
        let b = self.res.get(id)?;
        Ok((b.kind(), b.width(), b.height(), b.map_info()))
    }

    pub fn buf_get(&self, id: BufId, x: i32, y: i32) -> Result<Option<f64>, GfxError> {
        Ok(self.res.get(id)?.get(x, y))
    }

    pub fn buf_set(&mut self, id: BufId, x: i32, y: i32, v: f64) -> Result<(), GfxError> {
        self.res.get_mut(id)?.set(x, y, v);
        Ok(())
    }

    pub fn buf_fill(&mut self, id: BufId, v: f64) -> Result<(), GfxError> {
        self.res.get_mut(id)?.fill(v);
        Ok(())
    }

    /// `dst:copy(src, ...)`, or `dst:blit(src, ...)` when `honour_clip`,
    /// which applies the pen's clip if `dst` is the current target.
    #[allow(clippy::too_many_arguments)]
    pub fn buf_copy(
        &mut self,
        dst: BufId,
        src: BufId,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
        honour_clip: bool,
    ) -> Result<(), GfxError> {
        let clip = if honour_clip && dst == self.pen.target {
            Some(self.pen.clip)
        } else {
            None
        };
        if dst == src {
            self.res
                .get_mut(dst)?
                .copy_within(sx, sy, w, h, dx, dy, clip);
            return Ok(());
        }
        let (d, s) = self.res.pair_mut(dst, src)?;
        Ok(d.copy_from(s, sx, sy, w, h, dx, dy, clip)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::DEFAULT_PALETTE;
    use crate::snapshot::SnapshotLimits;

    fn cart_with(entries: Vec<(&str, Vec<u8>)>) -> Rc<dyn CartSource> {
        Rc::new(Snapshot::from_entries(entries, SnapshotLimits::default()).unwrap())
    }

    fn sheet_png() -> Vec<u8> {
        // 8x8: top-left pixel 1, the rest 0.
        let mut px = vec![0u8; 64];
        px[0] = 1;
        assets::encode_indexed_png(8, 8, &px, &DEFAULT_PALETTE)
    }

    #[test]
    fn default_state_has_a_screen_target_and_full_clip() {
        let mut d = DrawState::default();
        assert_eq!(d.screen_pixels().len(), 320 * 240);
        d.cls(3);
        assert!(d.screen_pixels().iter().all(|&p| p == 3));
        assert_eq!(d.pen.clip, Rect::new(0, 0, 320, 240));
    }

    #[test]
    fn draw_target_switches_and_resets_the_clip() {
        let mut d = DrawState::new(8, 8, Rc::new(Snapshot::empty()));
        let b = d.buf_alloc(BufKind::U8, 4, 4).unwrap();
        d.clip(Some(Rect::new(1, 1, 2, 2)));
        d.draw_target(Some(b)).unwrap();
        assert_eq!(d.pen.clip, Rect::new(0, 0, 4, 4));
        d.rectfill(0, 0, 10, 10, Colour::solid(5));
        assert!(d
            .res
            .get(b)
            .unwrap()
            .as_u8()
            .unwrap()
            .iter()
            .all(|&p| p == 5));
        assert!(d.screen_pixels().iter().all(|&p| p == 0));
        assert_eq!(d.pget(0, 0), 5);
        let m = d.buf_alloc(BufKind::I16, 2, 2).unwrap();
        assert_eq!(
            d.draw_target(Some(m)).unwrap_err().code(),
            "buf_kind_mismatch"
        );
        // Releasing the target falls back to the screen.
        d.buf_release(b).unwrap();
        assert_eq!(d.pen.target, BufId::SCREEN);
        d.pset(0, 0, Colour::solid(1));
        assert_eq!(d.screen_pixels()[0], 1);
    }

    #[test]
    fn clip_is_intersected_with_the_target() {
        let mut d = DrawState::new(8, 8, Rc::new(Snapshot::empty()));
        d.clip(Some(Rect::new(-2, -2, 6, 20)));
        assert_eq!(d.pen.clip, Rect::new(0, 0, 4, 8));
        d.clip(None);
        assert_eq!(d.pen.clip, Rect::new(0, 0, 8, 8));
    }

    #[test]
    fn load_sheet_caches_by_name_and_spr_needs_a_sheet() {
        let cart = cart_with(vec![("gfx/hero.png", sheet_png())]);
        let mut d = DrawState::new(8, 8, cart);
        assert_eq!(
            d.spr(0, 0, 0, 1, 1, false, false).unwrap_err().code(),
            "no_sheet"
        );
        let a = d.load_sheet("hero").unwrap();
        let b = d.load_sheet("hero").unwrap();
        assert_eq!(a, b);
        assert_eq!(d.res.ledger().used(), 64);
        assert_eq!(d.load_sheet("nope").unwrap_err().code(), "asset_not_found");
        assert_eq!(d.load_sheet("../x").unwrap_err().code(), "asset_invalid");
        d.sheet(Some(a)).unwrap();
        d.spr(0, 2, 2, 1, 1, false, false).unwrap();
        assert_eq!(d.pget(2, 2), 1);
        // Drawing the sheet onto itself is refused.
        d.draw_target(Some(a)).unwrap();
        assert_eq!(
            d.spr(0, 0, 0, 1, 1, false, false).unwrap_err().code(),
            "buf_aliased"
        );
        d.draw_target(None).unwrap();
        // Releasing the sheet deselects it and a reload decodes again.
        d.buf_release(a).unwrap();
        assert_eq!(d.pen.sheet, None);
        assert_eq!(d.res.ledger().used(), 0);
        let c = d.load_sheet("hero").unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn map_draws_through_the_sheet_and_checks_layers() {
        let json =
            br#"{"tile_size": 8, "width": 2, "height": 1, "layers": [[0, -1], [-1, 0]]}"#.to_vec();
        let cart = cart_with(vec![("gfx/t.png", sheet_png()), ("map/m.json", json)]);
        let mut d = DrawState::new(16, 8, cart);
        let m = d.load_map("m").unwrap();
        let s = d.load_sheet("t").unwrap();
        assert_eq!(
            d.map(m, 0, 0, 0, 0, 2, 1, 0).unwrap_err().code(),
            "no_sheet"
        );
        d.sheet(Some(s)).unwrap();
        assert_eq!(
            d.map(s, 0, 0, 0, 0, 2, 1, 0).unwrap_err().code(),
            "not_a_map"
        );
        d.map(m, 0, 0, 0, 0, 2, 1, 0).unwrap();
        assert_eq!(d.pget(0, 0), 1);
        assert_eq!(d.pget(8, 0), 0);
        d.cls(0);
        d.map(m, 0, 0, 0, 0, 2, 1, 1).unwrap();
        assert_eq!(d.pget(0, 0), 0);
        assert_eq!(d.pget(8, 0), 1);
        d.cls(0);
        d.map(m, 0, 0, 0, 0, 2, 1, 7).unwrap();
        assert!(d.screen_pixels().iter().all(|&p| p == 0));
        assert_eq!(d.sheet(Some(m)).unwrap_err().code(), "buf_kind_mismatch");
    }

    #[test]
    fn buf_copy_and_blit_honour_the_clip_only_on_the_target() {
        let mut d = DrawState::new(4, 4, Rc::new(Snapshot::empty()));
        let a = d.buf_alloc(BufKind::U8, 4, 4).unwrap();
        d.buf_fill(a, 7.0).unwrap();
        d.clip(Some(Rect::new(0, 0, 2, 2)));
        d.buf_copy(BufId::SCREEN, a, 0, 0, 4, 4, 0, 0, true)
            .unwrap();
        assert_eq!(d.screen_pixels().iter().filter(|&&p| p == 7).count(), 4);
        d.buf_copy(BufId::SCREEN, a, 0, 0, 4, 4, 0, 0, false)
            .unwrap();
        assert_eq!(d.screen_pixels().iter().filter(|&&p| p == 7).count(), 16);
        // Same-buffer copy is allowed.
        d.buf_set(a, 0, 0, 1.0).unwrap();
        d.buf_copy(a, a, 0, 0, 1, 1, 3, 3, false).unwrap();
        assert_eq!(d.buf_get(a, 3, 3).unwrap(), Some(1.0));
        // Errors keep their codes.
        let m = d.buf_alloc(BufKind::I16, 1, 1).unwrap();
        assert_eq!(
            d.buf_copy(a, m, 0, 0, 1, 1, 0, 0, false)
                .unwrap_err()
                .code(),
            "buf_kind_mismatch"
        );
        d.buf_release(a).unwrap();
        assert_eq!(d.buf_get(a, 0, 0).unwrap_err().code(), "buf_released");
        assert_eq!(
            d.buf_release(BufId::SCREEN).unwrap_err().code(),
            "buf_protected"
        );
    }

    #[test]
    fn reap_frees_only_live_ids() {
        let mut d = DrawState::new(4, 4, Rc::new(Snapshot::empty()));
        let a = d.buf_alloc(BufKind::U8, 4, 4).unwrap();
        d.buf_release(a).unwrap();
        let b = d.buf_alloc(BufKind::U8, 4, 4).unwrap();
        d.reap([a, b, BufId::SCREEN]);
        assert_eq!(d.res.live_count(), 1);
        assert_eq!(d.res.ledger().used(), 0);
    }

    #[test]
    fn pal_respects_the_locked_system_colours() {
        let mut d = DrawState::default();
        assert_eq!(
            d.pal(3, [1, 2, 3]).unwrap_err().code(),
            "palette_index_locked"
        );
        assert_eq!(
            d.pal(128, [1, 2, 3]).unwrap_err().code(),
            "palette_index_out_of_range"
        );
        d.pal(16, [1, 2, 3]).unwrap();
        assert_eq!(d.palette.get(16), [1, 2, 3]);
        d.pal_restore();
        assert_eq!(d.palette.get(16), DEFAULT_PALETTE[16]);
    }

    #[test]
    fn log_is_bounded() {
        let mut d = DrawState::default();
        for i in 0..300 {
            d.log_line(format!("line {i}"));
        }
        assert_eq!(d.log.len(), MAX_LOG_LINES + 1);
        assert!(d.log.last().unwrap().contains("truncated"));
        let mut d = DrawState::default();
        d.log_line("x".repeat(5000));
        assert_eq!(d.log[0].len(), MAX_LOG_LINE_BYTES + 3);
        d.log_line(format!("{}\u{e9}", "y".repeat(MAX_LOG_LINE_BYTES - 1)));
        assert!(d.log[1].ends_with("..."));
    }

    /// The meter prices pixels touched after clipping, so the counts
    /// every primitive returns are part of the API.
    #[test]
    fn primitives_report_pixels_touched_after_clipping() {
        let mut d = DrawState::default();
        assert_eq!(d.cls(0), 320 * 240);
        d.clip(Some(Rect::new(0, 0, 10, 10)));
        assert_eq!(d.cls(1), 100);
        assert_eq!(d.rectfill(-5, -5, 4, 4, Colour::solid(2)), 25);
        assert_eq!(d.rectfill(100, 100, 200, 200, Colour::solid(2)), 0);
        assert_eq!(d.pset(3, 3, Colour::solid(2)), 1);
        assert_eq!(d.pset(30, 3, Colour::solid(2)), 0);
        assert_eq!(d.line(0, 0, 99, 0, Colour::solid(2)), 10);
        assert_eq!(d.rect(0, 0, 9, 9, Colour::solid(2)), 36);
        // Circles floor at six per unit of radius, the unclipped walk.
        assert_eq!(d.circfill(5, 5, 1, Colour::solid(2)), 6);
        assert_eq!(d.circfill(5, 5, 4, Colour::solid(2)), 61);
        assert_eq!(d.circ(-40000, -40000, 40000, Colour::solid(2)), 6 * 32768);
        // A transparent source pixel is still touched; the pattern's
        // skipped pixels are too.
        d.palt(2, true);
        assert_eq!(d.rectfill(0, 0, 1, 1, Colour::solid(2)), 4);
        d.fillp(Fillp {
            pattern: 0xffff,
            transparent: true,
        });
        assert_eq!(d.rectfill(0, 0, 1, 1, Colour::solid(3)), 4);
        d.fillp(Fillp::default());
        let (end, touched) = d.print("ab", 0, 0, 7);
        assert_eq!(end, 8);
        assert!(touched > 0 && touched <= 30, "{touched}");
        d.clip(None);
        let sheet = d.res.alloc(BufKind::U8, 16, 8).unwrap();
        d.sheet(Some(sheet)).unwrap();
        assert_eq!(d.spr(0, 0, 0, 1, 1, false, false).unwrap(), 64);
        assert_eq!(d.spr(0, -4, 0, 1, 1, false, false).unwrap(), 32);
        assert_eq!(d.sspr(0, 0, 8, 8, 0, 0, 4, 4, false, false).unwrap(), 16);
    }
}
