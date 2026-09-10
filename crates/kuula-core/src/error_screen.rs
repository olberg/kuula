//! The Rust-drawn error screen: a system-palette
//! panel with the fault's code, location and message over the last
//! complete frame, and the two choices the host offers. There is no shell
//! yet, so this is what a person sees when a cart dies; it stays as the
//! fallback once the shell draws its own.
//!
//! It is host-side presentation: the headless host writes the raw frame,
//! so a fault's hash does not depend on this text.

use crate::blit;
use crate::buf::Rect;
use crate::fault::Fault;
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::pen::{Colour, Pen};
use crate::raster::{self, Surface};
use crate::resources::BufId;

const PANEL: u8 = 1; // midnight
const BORDER: u8 = 6; // mist
const CODE: u8 = 15; // peach
const LOCATION: u8 = 6; // mist
const MESSAGE: u8 = 7; // ivory
const CHOICES: u8 = 12; // sky

/// Longest message lines kept.
const MESSAGE_LINES: usize = 4;
const MARGIN: i32 = 8;

pub const CHOICES_TEXT: &str = "A restart   B quit";

/// The text lines of the panel, wrapped to `columns` characters.
pub fn lines(fault: &Fault, columns: usize) -> Vec<(String, u8)> {
    let columns = columns.max(8);
    let mut out = vec![(fault.code.clone(), CODE), (fault.location(), LOCATION)];
    let mut wrapped = wrap(&fault.message, columns);
    if wrapped.len() > MESSAGE_LINES {
        wrapped.truncate(MESSAGE_LINES);
        let last = wrapped.last_mut().expect("kept some");
        let keep = columns.saturating_sub(3);
        let mut cut = keep.min(last.len());
        while !last.is_char_boundary(cut) {
            cut -= 1;
        }
        last.truncate(cut);
        last.push_str("...");
    }
    out.extend(wrapped.into_iter().map(|l| (l, MESSAGE)));
    out.push((String::new(), MESSAGE));
    out.push((CHOICES_TEXT.to_string(), CHOICES));
    out
}

/// Greedy word wrap; a word longer than a line is split.
fn wrap(text: &str, columns: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word;
            while !word.is_empty() {
                if line.is_empty() {
                    if word.chars().count() <= columns {
                        line.push_str(word);
                        break;
                    }
                    let cut = word
                        .char_indices()
                        .nth(columns)
                        .map(|(i, _)| i)
                        .unwrap_or(word.len());
                    lines.push(word[..cut].to_string());
                    word = &word[cut..];
                } else if line.chars().count() + 1 + word.chars().count() <= columns {
                    line.push(' ');
                    line.push_str(word);
                    break;
                } else {
                    lines.push(std::mem::take(&mut line));
                }
            }
        }
        if !line.is_empty() || paragraph.is_empty() {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Draw the panel into `pixels`, a `width` by `height` screen. The frame
/// underneath is kept outside the panel.
pub fn compose(pixels: &mut [u8], width: u32, height: u32, fault: &Fault) {
    let (w, h) = (width as i32, height as i32);
    if w <= 0 || h <= 0 || pixels.len() < (w * h) as usize {
        return;
    }
    let mut s = Surface {
        pixels,
        width: w,
        height: h,
        touched: 0,
    };
    let pen = Pen::new(BufId::SCREEN, width, height);
    let columns = ((w - 4 * MARGIN) / GLYPH_WIDTH).max(8) as usize;
    let text = lines(fault, columns);
    let text_h = text.len() as i32 * GLYPH_HEIGHT;
    let panel = Rect::new(
        MARGIN,
        ((h - text_h) / 2 - MARGIN).max(0),
        w - 2 * MARGIN,
        (text_h + 2 * MARGIN).min(h),
    );
    raster::rectfill(
        &mut s,
        &pen,
        panel.x,
        panel.y,
        panel.right() - 1,
        panel.bottom() - 1,
        Colour::solid(PANEL),
    );
    raster::rect(
        &mut s,
        &pen,
        panel.x,
        panel.y,
        panel.right() - 1,
        panel.bottom() - 1,
        Colour::solid(BORDER),
    );
    let mut y = panel.y + MARGIN;
    for (line, colour) in &text {
        blit::print(&mut s, &pen, line, panel.x + MARGIN, y, *colour);
        y += GLYPH_HEIGHT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_and_splits_long_ones() {
        assert_eq!(wrap("a bb ccc", 5), ["a bb", "ccc"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("x\n\ny", 4), ["x", "", "y"]);
        assert_eq!(wrap("", 4), [""]);
        assert_eq!(wrap("äöäöäö ok", 4), ["äöäö", "äö", "ok"]);
    }

    #[test]
    fn lines_have_code_location_message_and_choices() {
        let f = Fault::new(
            "budget_exceeded",
            "main.lua",
            Some(7),
            "_update used 1 of 0 cycles",
        );
        let l = lines(&f, 40);
        assert_eq!(l[0], ("budget_exceeded".to_string(), CODE));
        assert_eq!(l[1], ("main.lua:7".to_string(), LOCATION));
        assert_eq!(l[2].0, "_update used 1 of 0 cycles");
        assert_eq!(l.last().unwrap().0, CHOICES_TEXT);
        let long = Fault::new("runtime_error", "main.lua", None, "word ".repeat(100));
        let l = lines(&long, 20);
        assert_eq!(l.len(), 2 + MESSAGE_LINES + 2);
        assert!(l[1 + MESSAGE_LINES].0.ends_with("..."));
    }

    #[test]
    fn compose_keeps_the_frame_outside_the_panel() {
        let (w, h) = (320u32, 240u32);
        let mut px = vec![9u8; (w * h) as usize];
        let f = Fault::new("runtime_error", "main.lua", Some(3), "boom");
        compose(&mut px, w, h, &f);
        assert_eq!(px[0], 9, "corner untouched");
        assert_eq!(px[(h as usize - 1) * w as usize], 9);
        let mid = (h as usize / 2) * w as usize + w as usize / 2;
        assert!(
            px[mid] == PANEL || px[mid] == MESSAGE || px[mid] == LOCATION || px[mid] == CODE,
            "panel drawn in the middle: {}",
            px[mid]
        );
        assert!(px.contains(&CODE), "the code is drawn in peach");
        assert!(px.contains(&CHOICES));
        // A tiny screen does not panic.
        let mut tiny = vec![0u8; 4];
        compose(&mut tiny, 2, 2, &f);
        compose(&mut [], 0, 0, &f);
    }
}
