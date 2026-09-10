//! Bounded asset decoders: `gfx/*.png` into `u8` sheets and
//! `map/*.json` into `i16` map buffers. Dimensions are validated and the
//! ledger charged before any image data is decoded.

use std::collections::HashMap;
use std::io::Cursor;

use serde::Deserialize;

use crate::buf::{Buf, BufKind, MapInfo, MAX_MAP_DIM, MAX_MAP_LAYERS, MAX_SHEET_DIM};
use crate::palette::DEFAULT_PALETTE;
use crate::resources::{BufId, GfxError, Resources};

/// Most bytes the PNG decoder may allocate for one image.
const PNG_DECODER_LIMIT: usize = 8 * 1024 * 1024;

pub fn sheet_path(name: &str) -> String {
    format!("gfx/{name}.png")
}

pub fn map_path(name: &str) -> String {
    format!("map/{name}.json")
}

/// Decode a PNG into a sheet and put it in the slab. `path` is only for
/// messages.
pub fn decode_sheet(path: &str, bytes: &[u8], res: &mut Resources) -> Result<BufId, GfxError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: PNG_DECODER_LIMIT,
    });
    decoder.set_transformations(png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| GfxError::asset_invalid(path, format!("not a readable PNG: {e}")))?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    if width == 0 || height == 0 || width > MAX_SHEET_DIM || height > MAX_SHEET_DIM {
        return Err(GfxError::asset_too_large(
            path,
            format!("{width}x{height} is not within 1..={MAX_SHEET_DIM} on each side"),
        ));
    }
    let pixels = width as usize * height as usize;
    res.reserve(pixels)?;
    let decoded = (|| {
        let mut raw = vec![0u8; reader.output_buffer_size()];
        let out = reader
            .next_frame(&mut raw)
            .map_err(|e| GfxError::asset_invalid(path, format!("PNG data: {e}")))?;
        raw.truncate(out.buffer_size());
        // Colour type and depth of the *output* samples: `STRIP_16` has
        // already turned 16-bit channels into 8-bit ones by now.
        to_indices(
            path,
            &raw,
            width,
            height,
            out.color_type,
            out.bit_depth as u8,
        )
    })();
    match decoded {
        Ok(indices) => Ok(res.insert_charged(Buf::from_u8(width, height, indices))),
        Err(e) => {
            res.unreserve(pixels);
            Err(e)
        }
    }
}

fn to_indices(
    path: &str,
    raw: &[u8],
    width: u32,
    height: u32,
    colour: png::ColorType,
    depth: u8,
) -> Result<Vec<u8>, GfxError> {
    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(w * h);
    if !matches!(depth, 1 | 2 | 4 | 8) {
        return Err(GfxError::asset_invalid(
            path,
            format!("unsupported sample depth {depth}"),
        ));
    }
    match colour {
        png::ColorType::Indexed | png::ColorType::Grayscale => {
            // Packed rows for depths below 8, each row byte-aligned.
            let per_byte = 8 / depth as usize;
            let row_bytes = w.div_ceil(per_byte);
            if raw.len() < row_bytes * h {
                return Err(GfxError::asset_invalid(path, "PNG data is short"));
            }
            for y in 0..h {
                let row = &raw[y * row_bytes..(y + 1) * row_bytes];
                for x in 0..w {
                    let v = if depth == 8 {
                        row[x]
                    } else {
                        let byte = row[x / per_byte];
                        let shift = 8 - depth as usize * (x % per_byte + 1);
                        (byte >> shift) & ((1u8 << depth) - 1)
                    };
                    out.push(v & 0x7f);
                }
            }
        }
        png::ColorType::Rgb | png::ColorType::Rgba | png::ColorType::GrayscaleAlpha => {
            let channels = match colour {
                png::ColorType::Rgb => 3,
                png::ColorType::Rgba => 4,
                _ => 2,
            };
            if raw.len() < w * h * channels {
                return Err(GfxError::asset_invalid(path, "PNG data is short"));
            }
            let lookup = default_palette_lookup();
            for (i, px) in raw.chunks_exact(channels).take(w * h).enumerate() {
                let (rgb, alpha) = match colour {
                    png::ColorType::Rgb => ([px[0], px[1], px[2]], 255),
                    png::ColorType::Rgba => ([px[0], px[1], px[2]], px[3]),
                    _ => ([px[0], px[0], px[0]], px[1]),
                };
                if alpha < 128 {
                    out.push(0);
                    continue;
                }
                match lookup.get(&rgb) {
                    Some(&c) => out.push(c),
                    None => {
                        return Err(GfxError::asset_invalid(
                            path,
                            format!(
                                "pixel {} at ({}, {}) is #{:02x}{:02x}{:02x}, not a default palette colour; use an indexed PNG",
                                i,
                                i % w,
                                i / w,
                                rgb[0],
                                rgb[1],
                                rgb[2]
                            ),
                        ))
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Exact RGB to index over the default palette, first occurrence wins.
fn default_palette_lookup() -> HashMap<[u8; 3], u8> {
    let mut m = HashMap::with_capacity(DEFAULT_PALETTE.len());
    for (i, rgb) in DEFAULT_PALETTE.iter().enumerate() {
        m.entry(*rgb).or_insert(i as u8);
    }
    m
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MapFile {
    tile_size: u32,
    width: u32,
    height: u32,
    layers: Vec<Vec<i32>>,
}

/// Decode a map JSON into one `i16` buffer, `width` wide and
/// `height * layers` tall, and put it in the slab.
pub fn decode_map(path: &str, bytes: &[u8], res: &mut Resources) -> Result<BufId, GfxError> {
    let file: MapFile = serde_json::from_slice(bytes)
        .map_err(|e| GfxError::asset_invalid(path, format!("map JSON: {e}")))?;
    if file.tile_size != 8 && file.tile_size != 16 {
        return Err(GfxError::asset_invalid(
            path,
            format!("tile_size must be 8 or 16, got {}", file.tile_size),
        ));
    }
    if file.width == 0 || file.height == 0 || file.width > MAX_MAP_DIM || file.height > MAX_MAP_DIM
    {
        return Err(GfxError::asset_too_large(
            path,
            format!(
                "{}x{} cells is not within 1..={MAX_MAP_DIM} on each side",
                file.width, file.height
            ),
        ));
    }
    let layers = file.layers.len() as u32;
    if layers == 0 || layers > MAX_MAP_LAYERS {
        return Err(GfxError::asset_invalid(
            path,
            format!("maps have 1 to {MAX_MAP_LAYERS} layers, got {layers}"),
        ));
    }
    let per_layer = file.width as usize * file.height as usize;
    let mut cells = Vec::with_capacity(per_layer * layers as usize);
    for (li, layer) in file.layers.iter().enumerate() {
        if layer.len() != per_layer {
            return Err(GfxError::asset_invalid(
                path,
                format!("layer {li} has {} cells, expected {per_layer}", layer.len()),
            ));
        }
        for (ci, &v) in layer.iter().enumerate() {
            let v = i16::try_from(v).map_err(|_| {
                GfxError::asset_invalid(
                    path,
                    format!("layer {li} cell {ci} value {v} is not an i16"),
                )
            })?;
            cells.push(v);
        }
    }
    let bytes = cells.len() * 2;
    res.reserve(bytes)?;
    let buf = Buf::from_i16(
        file.width,
        file.height * layers,
        cells,
        Some(MapInfo {
            rows: file.height,
            layers,
            tile_size: file.tile_size,
        }),
    );
    debug_assert_eq!(buf.bytes(), bytes);
    debug_assert_eq!(buf.kind(), BufKind::I16);
    Ok(res.insert_charged(buf))
}

/// Encode an indexed 8-bit PNG with a 128-entry palette. Used by the
/// headless host for captures and by tests to make fixtures; kept here
/// so the decoder and encoder agree.
pub fn encode_indexed_png(width: u32, height: u32, pixels: &[u8], palette: &[[u8; 3]]) -> Vec<u8> {
    assert_eq!(pixels.len(), width as usize * height as usize);
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        let plte: Vec<u8> = palette.iter().flat_map(|c| c.iter().copied()).collect();
        enc.set_palette(plte);
        let mut w = enc.write_header().expect("in-memory PNG header");
        w.write_image_data(pixels).expect("in-memory PNG data");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(rgba).unwrap();
        drop(w);
        out
    }

    #[test]
    fn indexed_and_rgba_decode_to_the_same_indices() {
        let pixels = [0u8, 7, 8, 0x7f, 16, 127];
        let indexed = encode_indexed_png(3, 2, &pixels, &DEFAULT_PALETTE);
        let mut rgba = Vec::new();
        for &p in &pixels {
            rgba.extend_from_slice(&DEFAULT_PALETTE[p as usize]);
            rgba.push(255);
        }
        let rgba = rgba_png(3, 2, &rgba);

        let mut res = Resources::new(1, 1);
        let a = decode_sheet("gfx/a.png", &indexed, &mut res).unwrap();
        let b = decode_sheet("gfx/b.png", &rgba, &mut res).unwrap();
        assert_eq!(res.get(a).unwrap().as_u8().unwrap(), &pixels);
        // Index 16 is black like index 0, so it decodes as 0 from RGBA.
        let mut expected = pixels;
        expected[4] = 0;
        assert_eq!(res.get(b).unwrap().as_u8().unwrap(), &expected);
        assert_eq!(res.ledger().used(), 12);
    }

    #[test]
    fn transparent_rgba_pixels_become_colour_0_and_unknown_colours_fail() {
        let mut res = Resources::new(1, 1);
        let png = rgba_png(2, 1, &[255, 0, 77, 255, 255, 0, 77, 10]);
        let id = decode_sheet("gfx/t.png", &png, &mut res).unwrap();
        assert_eq!(res.get(id).unwrap().as_u8().unwrap(), &[8, 0]);
        let png = rgba_png(1, 1, &[1, 2, 3, 255]);
        let e = decode_sheet("gfx/u.png", &png, &mut res).unwrap_err();
        assert_eq!(e.code(), "asset_invalid");
        assert!(e.to_string().contains("#010203"), "{e}");
        assert_eq!(res.ledger().used(), 2, "failed decode released its bytes");
    }

    #[test]
    fn packed_indexed_depths_unpack() {
        // 2-bit indexed, 5 pixels wide: values 0,1,2,3,1.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 5, 1);
            enc.set_color(png::ColorType::Indexed);
            enc.set_depth(png::BitDepth::Two);
            enc.set_palette(vec![0u8; 12]);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[0b0001_1011, 0b0100_0000]).unwrap();
        }
        let mut res = Resources::new(1, 1);
        let id = decode_sheet("gfx/p.png", &out, &mut res).unwrap();
        assert_eq!(res.get(id).unwrap().as_u8().unwrap(), &[0, 1, 2, 3, 1]);
    }

    #[test]
    fn sixteen_bit_pngs_decode_through_the_stripped_depth() {
        // 16-bit grayscale, 2 pixels: 0x0100 and 0x8300. STRIP_16 keeps
        // the high byte, so these are 1 and 0x83 & 0x7f = 3.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Sixteen);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[0x01, 0x00, 0x83, 0x00]).unwrap();
        }
        let mut res = Resources::new(1, 1);
        let id = decode_sheet("gfx/deep.png", &out, &mut res).unwrap();
        assert_eq!(res.get(id).unwrap().as_u8().unwrap(), &[1, 3]);
    }

    #[test]
    fn oversized_and_garbage_pngs_fail_before_charging() {
        let mut res = Resources::new(1, 1);
        let big = encode_indexed_png(1025, 1, &vec![0; 1025], &DEFAULT_PALETTE);
        let e = decode_sheet("gfx/big.png", &big, &mut res).unwrap_err();
        assert_eq!(e.code(), "asset_too_large");
        let e = decode_sheet("gfx/no.png", b"not a png", &mut res).unwrap_err();
        assert_eq!(e.code(), "asset_invalid");
        // A truncated file fails after the header; bytes are released.
        let ok = encode_indexed_png(4, 4, &[1; 16], &DEFAULT_PALETTE);
        let e = decode_sheet("gfx/cut.png", &ok[..ok.len() - 20], &mut res).unwrap_err();
        assert_eq!(e.code(), "asset_invalid");
        assert_eq!(res.ledger().used(), 0);
    }

    #[test]
    fn map_decodes_layers_into_one_buffer() {
        let json = br#"{"tile_size": 8, "width": 2, "height": 2, "layers": [[1, 2, 3, 4], [-1, 0, 0, -1]]}"#;
        let mut res = Resources::new(1, 1);
        let id = decode_map("map/m.json", json, &mut res).unwrap();
        let b = res.get(id).unwrap();
        assert_eq!(b.width(), 2);
        assert_eq!(b.height(), 4);
        assert_eq!(b.as_i16().unwrap(), &[1, 2, 3, 4, -1, 0, 0, -1]);
        assert_eq!(
            b.map_info(),
            Some(MapInfo {
                rows: 2,
                layers: 2,
                tile_size: 8
            })
        );
        assert_eq!(res.ledger().used(), 16);
    }

    #[test]
    fn map_errors_have_stable_codes() {
        let mut res = Resources::new(1, 1);
        let cases: [(&[u8], &str); 7] = [
            (
                br#"{"tile_size": 8, "width": 2, "height": 1, "layers": [[1]]}"#,
                "asset_invalid",
            ),
            (
                br#"{"tile_size": 12, "width": 1, "height": 1, "layers": [[1]]}"#,
                "asset_invalid",
            ),
            (
                br#"{"tile_size": 8, "width": 257, "height": 1, "layers": [[1]]}"#,
                "asset_too_large",
            ),
            (
                br#"{"tile_size": 8, "width": 1, "height": 1, "layers": []}"#,
                "asset_invalid",
            ),
            (
                br#"{"tile_size": 8, "width": 1, "height": 1, "layers": [[1],[1],[1],[1],[1]]}"#,
                "asset_invalid",
            ),
            (
                br#"{"tile_size": 8, "width": 1, "height": 1, "layers": [[70000]]}"#,
                "asset_invalid",
            ),
            (
                br#"{"tile_size": 8, "width": 1, "height": 1, "layers": [[1]], "extra": 1}"#,
                "asset_invalid",
            ),
        ];
        for (json, code) in cases {
            let e = decode_map("map/x.json", json, &mut res).unwrap_err();
            assert_eq!(e.code(), code, "{}", String::from_utf8_lossy(json));
        }
        assert_eq!(res.ledger().used(), 0);
    }
}
