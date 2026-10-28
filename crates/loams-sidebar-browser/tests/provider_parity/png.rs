//! Enough of a PNG decoder to tell a rendered page from a blank one.
//!
//! The crate itself reads only the `IHDR` (see `frame.rs`), which gives the
//! dimensions but says nothing about the pixels. The screenshot scenario needs
//! both, so this decodes 8-bit non-interlaced greyscale, RGB, grey+alpha and
//! RGBA images: inflate the `IDAT` stream and undo the five scanline filters.
//! Anything else is reported as unsupported rather than guessed at.

use std::io::Read as _;

/// A decoded image, as RGBA8.
#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The pixel at `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ]
    }

    /// The share of pixels that differ from the most common colour.
    pub fn non_background_share(&self) -> f64 {
        let mut counts = std::collections::HashMap::<[u8; 4], u64>::new();
        for chunk in self.rgba.chunks_exact(4) {
            *counts
                .entry([chunk[0], chunk[1], chunk[2], chunk[3]])
                .or_default() += 1;
        }
        let total: u64 = counts.values().sum();
        let background = counts.values().copied().max().unwrap_or(0);
        if total == 0 {
            return 0.0;
        }
        (total - background) as f64 / total as f64
    }

    /// How many pixels are strongly red, the fixture block's colour (#d01c1c).
    pub fn reddish_pixels(&self) -> u64 {
        self.rgba
            .chunks_exact(4)
            .filter(|p| p[0] > 150 && p[1] < 90 && p[2] < 90)
            .count() as u64
    }
}

/// Decode a PNG.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Err("not a PNG".to_string());
    }
    let mut at = 8;
    let mut header: Option<(u32, u32, u8, u8, u8)> = None;
    let mut idat = Vec::new();
    while at + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let kind = &bytes[at + 4..at + 8];
        let data_start = at + 8;
        let data_end = data_start
            .checked_add(length)
            .filter(|end| *end + 4 <= bytes.len())
            .ok_or("a truncated chunk")?;
        let data = &bytes[data_start..data_end];
        match kind {
            b"IHDR" if data.len() >= 13 => {
                let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                header = Some((width, height, data[8], data[9], data[12]));
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at = data_end + 4;
    }
    let (width, height, depth, color, interlace) = header.ok_or("no IHDR")?;
    if depth != 8 || interlace != 0 {
        return Err(format!(
            "unsupported PNG: bit depth {depth}, interlace {interlace}"
        ));
    }
    let channels = match color {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        other => return Err(format!("unsupported PNG colour type {other}")),
    };
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return Err(format!("implausible dimensions {width}x{height}"));
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(idat.as_slice())
        .read_to_end(&mut raw)
        .map_err(|error| format!("inflating IDAT: {error}"))?;
    let stride = width as usize * channels;
    if raw.len() < (stride + 1) * height as usize {
        return Err("IDAT is shorter than the image".to_string());
    }
    let mut pixels = vec![0_u8; stride * height as usize];
    for row in 0..height as usize {
        let filter = raw[row * (stride + 1)];
        let line = &raw[row * (stride + 1) + 1..(row + 1) * (stride + 1)];
        for (i, &x) in line.iter().enumerate() {
            let a = if i >= channels {
                pixels[row * stride + i - channels]
            } else {
                0
            };
            let b = if row > 0 {
                pixels[(row - 1) * stride + i]
            } else {
                0
            };
            let c = if row > 0 && i >= channels {
                pixels[(row - 1) * stride + i - channels]
            } else {
                0
            };
            pixels[row * stride + i] = match filter {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                4 => x.wrapping_add(paeth(a, b, c)),
                other => return Err(format!("unknown scanline filter {other}")),
            };
        }
    }
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for px in pixels.chunks_exact(channels) {
        match channels {
            1 => rgba.extend_from_slice(&[px[0], px[0], px[0], 255]),
            2 => rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]),
            3 => rgba.extend_from_slice(&[px[0], px[1], px[2], 255]),
            _ => rgba.extend_from_slice(px),
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let pa = (p - i16::from(a)).abs();
    let pb = (p - i16::from(b)).abs();
    let pc = (p - i16::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// A 2x2 RGB image, filter 0, built by hand: red, white / white, white.
    fn tiny() -> Vec<u8> {
        let raw = [0, 255, 0, 0, 255, 255, 255, 0, 255, 255, 255, 255, 255, 255];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw).expect("compress");
        let idat = encoder.finish().expect("finish");
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0, 0, 0, 0]);
        };
        chunk(b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 2, 8, 2, 0, 0, 0]);
        chunk(b"IDAT", &idat);
        chunk(b"IEND", &[]);
        out
    }

    #[test]
    fn decodes_a_hand_built_png() {
        let image = decode(&tiny()).expect("decodes");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 1), [255, 255, 255, 255]);
        assert_eq!(image.reddish_pixels(), 1);
        assert!((image.non_background_share() - 0.25).abs() < 1e-9);
    }
}
