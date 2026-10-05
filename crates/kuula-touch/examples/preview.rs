//! Writes PNGs of the whole composed window for a few phone screens, each
//! with the controls of a cart with two buttons and of one with all of
//! them, idle and with some controls held: the picture is a generated 320
//! by 240 test frame, so its edges and its centring can be seen.
//!
//! `cargo run -p kuula-touch --example preview -- <out dir>`

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use kuula_core::input::{BASE_BUTTONS, CART_BUTTONS};
use kuula_core::PALETTE_SIZE;
use kuula_touch::{draw_controls, present, Canvas, Insets, Layout, Pad};

const W: usize = 320;
const H: usize = 240;

/// A gradient of blues for the sky, bands of colour under it, a one pixel
/// border, and a diagonal and a cross so that scaling is visible.
fn test_frame() -> Vec<u8> {
    let mut f = vec![0u8; W * H];
    for y in 0..H {
        for x in 0..W {
            f[y * W + x] = match y {
                0..=79 => 1 + (y / 20) as u8,
                80..=119 => 5 + (x / 40) as u8,
                120..=159 => 14,
                _ => 15 + ((x + y) / 16 % 4) as u8,
            };
        }
    }
    for i in 0..W.min(H) {
        f[i * W + i] = 20;
    }
    for x in 0..W {
        f[(H / 2) * W + x] = 21;
        f[x] = 22;
        f[(H - 1) * W + x] = 22;
    }
    for y in 0..H {
        f[y * W + W / 2] = 21;
        f[y * W] = 22;
        f[y * W + W - 1] = 22;
    }
    f
}

fn palette() -> [[u8; 3]; PALETTE_SIZE] {
    let mut p = [[0u8; 3]; PALETTE_SIZE];
    let colours: [[u8; 3]; 23] = [
        [0, 0, 0],
        [20, 30, 90],
        [30, 50, 130],
        [50, 80, 170],
        [80, 120, 200],
        [200, 60, 60],
        [220, 140, 40],
        [230, 210, 60],
        [90, 190, 80],
        [40, 150, 120],
        [60, 100, 200],
        [130, 70, 190],
        [200, 80, 170],
        [240, 240, 240],
        [40, 40, 44],
        [150, 100, 60],
        [120, 80, 50],
        [90, 60, 40],
        [60, 40, 30],
        [0, 0, 0],
        [255, 255, 255],
        [255, 255, 0],
        [255, 0, 0],
    ];
    p[..colours.len()].copy_from_slice(&colours);
    p
}

fn write_png(path: &Path, data: &[u8], w: usize, h: usize) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(data).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let out = std::env::args().nth(1).ok_or("usage: preview <out dir>")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;

    let frame = test_frame();
    let palette = palette();
    // Window size, density and the insets of a typical cutout and gesture
    // bar, for landscape and then portrait.
    let landscape = Insets {
        left: 100,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let portrait = Insets {
        left: 0,
        top: 80,
        right: 0,
        bottom: 48,
    };
    let windows: [((i32, i32), f32, Insets); 6] = [
        ((2400, 1080), 2.75, landscape),
        ((1920, 1080), 2.625, Insets::default()),
        ((1280, 720), 2.0, Insets::default()),
        ((1080, 2400), 2.75, portrait),
        ((1080, 1920), 2.625, Insets::default()),
        ((720, 1280), 2.0, portrait),
    ];
    let tiers = [("", BASE_BUTTONS), ("all-", CART_BUTTONS)];
    for (((w, h), density, insets), (tier, shown)) in
        windows.into_iter().flat_map(|w| tiers.map(|t| (w, t)))
    {
        let layout = Layout::new((w, h), insets, density, (W as u32, H as u32)).showing(shown);
        // The fingers: one steering the D-pad up and to the right, one in
        // the gap between A and B, one on Menu, and one on every second
        // pill of a cart with all the buttons.
        let mut pad = Pad::new();
        for (id, (_, key)) in layout.keys().step_by(2).enumerate() {
            pad.down(
                &layout,
                10 + id as u64,
                (key.x + key.w / 2) as f32,
                (key.y + key.h / 2) as f32,
            );
        }
        let dpad = layout.dpad();
        let reach = dpad.r as f32 * 0.8;
        pad.down(&layout, 1, dpad.cx as f32 + reach, dpad.cy as f32 - reach);
        let (a, b) = (layout.button_a(), layout.button_b());
        pad.down(
            &layout,
            2,
            (a.cx + b.cx) as f32 / 2.0,
            (a.cy + b.cy) as f32 / 2.0,
        );
        let menu = layout.menu();
        pad.down(
            &layout,
            3,
            (menu.x + menu.w / 2) as f32,
            (menu.y + menu.h / 2) as f32,
        );
        let held = pad.buttons();

        for (name, pressed) in [("idle", 0), ("held", held)] {
            let (cw, ch) = (w as usize, h as usize);
            let mut data = vec![0x5a; cw * ch * 4];
            let mut canvas = Canvas {
                data: &mut data,
                width: cw,
                height: ch,
                stride: cw,
            };
            present(&mut canvas, &layout, &frame, &palette, W as u32, H as u32);
            draw_controls(&mut canvas, &layout, pressed);
            let file = out.join(format!("{w}x{h}-{tier}{name}.png"));
            write_png(&file, &data, cw, ch)?;
            println!("{} (scale {})", file.display(), layout.scale());
        }
    }
    Ok(())
}
