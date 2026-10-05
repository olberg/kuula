//! The console's frame into the window buffer.

use kuula_core::PALETTE_SIZE;

use crate::canvas::{fill, Canvas, BLACK};
use crate::layout::Layout;

/// Draw the console's frame, `pixels` as palette indexes `width` to a row,
/// through `palette` into `canvas` at `layout.picture()`, each pixel a
/// `layout.scale()` square. Everything of the canvas outside the picture
/// is cleared to black. Bytes are R, G, B and X, with X written as 255.
///
/// A frame of another size than the layout was made for, or a `pixels`
/// that is too short, is drawn as far as it goes and the rest of the
/// picture is black; so is a canvas smaller than the layout, which is
/// clipped. An index of `PALETTE_SIZE` or more is black.
///
/// This runs for every frame and does no multiplication or division for a
/// pixel and no 64-bit arithmetic: each source row is scaled across into
/// the first of its lines in the canvas and the other lines are copied from
/// that one. Nothing is allocated; the one table is on the stack.
pub fn present(
    canvas: &mut Canvas,
    layout: &Layout,
    pixels: &[u8],
    palette: &[[u8; 3]; PALETTE_SIZE],
    width: u32,
    height: u32,
) {
    let (cw, ch) = canvas.visible();
    if cw == 0 || ch == 0 {
        return;
    }
    let mut lut = [BLACK; 256];
    for (slot, rgb) in lut.iter_mut().zip(palette) {
        *slot = [rgb[0], rgb[1], rgb[2], 255];
    }

    let picture = layout.picture();
    let scale = layout.scale().max(1) as usize;
    let (source_w, source_h) = layout.source();
    let cols = (width as usize).min(source_w as usize);
    let rows = (height as usize).min(source_h as usize);
    let pitch = width as usize;

    // The part of the picture that is inside the canvas.
    let clip_x = |v: i32| (v.max(0) as usize).min(cw);
    let clip_y = |v: i32| (v.max(0) as usize).min(ch);
    let (x0, x1) = (clip_x(picture.x), clip_x(picture.right()));
    let (y0, y1) = (clip_y(picture.y), clip_y(picture.bottom()));

    for y in 0..ch {
        let at = canvas.offset(0, y);
        let row = &mut canvas.data[at..at + cw * 4];
        if y < y0 || y >= y1 || x0 >= x1 {
            fill(row, BLACK);
        } else {
            fill(&mut row[..x0 * 4], BLACK);
            fill(&mut row[x1 * 4..], BLACK);
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let len = (x1 - x0) * 4;
    // How far into the picture the first visible line and column are.
    let skipped_lines = (y0 as i32 - picture.y) as usize;
    let skipped_columns = (x0 as i32 - picture.x) as usize;
    if layout.scale_halves() == 3 {
        // One and a half: three lines for every two rows of the frame, the
        // first row of a pair two lines high and the second one line.
        let (mut pair, mut line) = (skipped_lines / 3, skipped_lines % 3);
        for y in y0..y1 {
            let at = canvas.offset(x0, y);
            if line == 1 && y > y0 {
                let above = canvas.offset(x0, y - 1);
                canvas.data.copy_within(above..above + len, at);
            } else {
                let sy = pair + pair + usize::from(line == 2);
                let source = if sy < rows {
                    source_row(pixels, sy * pitch, cols)
                } else {
                    &[]
                };
                half_row(
                    &mut canvas.data[at..at + len],
                    source,
                    &lut,
                    skipped_columns,
                );
            }
            line += 1;
            if line == 3 {
                line = 0;
                pair += 1;
            }
        }
        return;
    }
    let (mut sy, mut line) = (skipped_lines / scale, skipped_lines % scale);
    for y in y0..y1 {
        let at = canvas.offset(x0, y);
        if line != 0 && y > y0 {
            let above = canvas.offset(x0, y - 1);
            canvas.data.copy_within(above..above + len, at);
        } else {
            let source = if sy < rows {
                source_row(pixels, sy * pitch, cols)
            } else {
                &[]
            };
            scale_row(
                &mut canvas.data[at..at + len],
                source,
                &lut,
                scale,
                skipped_columns,
            );
        }
        line += 1;
        if line == scale {
            line = 0;
            sy += 1;
        }
    }
}

/// Up to `cols` indexes of the source row starting at `start`; fewer when
/// `pixels` ends first.
fn source_row(pixels: &[u8], start: usize, cols: usize) -> &[u8] {
    match pixels.get(start..) {
        Some(rest) => &rest[..rest.len().min(cols)],
        None => &[],
    }
}

/// Fill `out`, a stretch of one line of the picture that begins `from`
/// pixels from the picture's left edge, with `source` at one and a half
/// times its width: of each pair of source pixels the first is two wide
/// and the second one. Whatever lies beyond the source is black.
fn half_row(out: &mut [u8], source: &[u8], lut: &[[u8; 4]; 256], from: usize) {
    // The source pixel under the first one drawn, and where in its group
    // of three that one is.
    let (group, mut place) = (from / 3, from % 3);
    let mut at = group + group + usize::from(place == 2);
    for p in out.as_chunks_mut::<4>().0 {
        *p = match source.get(at) {
            Some(&index) => lut[index as usize],
            None => BLACK,
        };
        // The first of a pair is drawn twice, the second once.
        if place != 0 {
            at += 1;
        }
        place += 1;
        if place == 3 {
            place = 0;
        }
    }
}

/// Fill `out`, a stretch of one line of the picture that begins `from`
/// pixels from the picture's left edge, with `source` scaled `scale` times
/// across; whatever lies beyond the source is black.
fn scale_row(out: &mut [u8], source: &[u8], lut: &[[u8; 4]; 256], scale: usize, from: usize) {
    let available = source.len() * scale;
    let drawn = available.saturating_sub(from).min(out.len() / 4);
    let (data, rest) = out.split_at_mut(drawn * 4);
    fill(rest, BLACK);
    if drawn == 0 {
        return;
    }
    let (first, skip) = (from / scale, from % scale);
    if skip == 0 && drawn.is_multiple_of(scale) {
        for (chunk, &index) in data.chunks_exact_mut(scale * 4).zip(&source[first..]) {
            let pixel = lut[index as usize];
            for p in chunk.as_chunks_mut::<4>().0 {
                *p = pixel;
            }
        }
    } else {
        // A stretch that starts or ends inside a source pixel: the edge of
        // a canvas smaller than the picture.
        let (mut at, mut left) = (first, scale - skip);
        for p in data.as_chunks_mut::<4>().0 {
            *p = lut[source[at] as usize];
            left -= 1;
            if left == 0 {
                at += 1;
                left = scale;
            }
        }
    }
}
