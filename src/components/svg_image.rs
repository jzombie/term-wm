use std::fs;
use std::io;
use std::path::Path;

use crate::components::Component;
use crate::components::ascii_image::AsciiImageComponent;
use crate::ui::UiFrame;

enum Pnm {
    Luma {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    Rgba {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
}

pub struct SvgImageComponent {
    inner: AsciiImageComponent,
}

impl Component for SvgImageComponent {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: ratatui::layout::Rect, focused: bool) {
        self.inner.render(frame, area, focused)
    }
}

impl SvgImageComponent {
    pub fn new() -> Self {
        Self {
            inner: AsciiImageComponent::new(),
        }
    }
    pub fn set_keep_aspect(&mut self, keep: bool) {
        self.inner.set_keep_aspect(keep);
    }
    pub fn set_colorize(&mut self, colorize: bool) {
        self.inner.set_colorize(colorize);
    }
    pub fn set_luma8(&mut self, width: u32, height: u32, luma: Vec<u8>) {
        self.inner.set_luma8(width, height, luma);
    }
    pub fn set_rgba8(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        self.inner.set_rgba8(width, height, rgba);
    }
    pub fn load_svg_from_path<P: AsRef<Path>>(&mut self, path: P) -> Result<(), String> {
        self.inner.load_svg_from_path(path)
    }
    pub fn load_from_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let p = path.as_ref();
        if let Some(ext) = p.extension().and_then(|s| s.to_str())
            && ext.eq_ignore_ascii_case("svg")
        {
            return self
                .inner
                .load_svg_from_path(p)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        }
        let bytes = fs::read(p)?;
        match decode_pnm(&bytes) {
            Some(Pnm::Luma {
                width,
                height,
                data,
            }) => {
                self.set_luma8(width, height, data);
                Ok(())
            }
            Some(Pnm::Rgba {
                width,
                height,
                data,
            }) => {
                self.set_rgba8(width, height, data);
                Ok(())
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported image",
            )),
        }
    }
}

fn decode_pnm(bytes: &[u8]) -> Option<Pnm> {
    let mut idx = 0usize;
    let magic = next_token(bytes, &mut idx)?;
    let width: u32 = next_token(bytes, &mut idx)?.parse().ok()?;
    let height: u32 = next_token(bytes, &mut idx)?.parse().ok()?;
    let maxval: u32 = next_token(bytes, &mut idx)?.parse().ok()?;
    if maxval == 0 || maxval > 255 {
        return None;
    }
    if magic == "P5" {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        let count = (width * height) as usize;
        let data = bytes.get(idx..idx + count)?.to_vec();
        if maxval != 255 {
            let data = data
                .into_iter()
                .map(|v| ((v as u32 * 255) / maxval) as u8)
                .collect();
            return Some(Pnm::Luma {
                width,
                height,
                data,
            });
        }
        return Some(Pnm::Luma {
            width,
            height,
            data,
        });
    }
    if magic == "P6" {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        let count = (width * height * 3) as usize;
        let raw = bytes.get(idx..idx + count)?.to_vec();
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for chunk in raw.chunks_exact(3) {
            let r = scale_max(chunk[0], maxval);
            let g = scale_max(chunk[1], maxval);
            let b = scale_max(chunk[2], maxval);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
        return Some(Pnm::Rgba {
            width,
            height,
            data: rgba,
        });
    }
    None
}

fn scale_max(value: u8, maxval: u32) -> u8 {
    if maxval == 255 {
        value
    } else {
        ((value as u32 * 255) / maxval) as u8
    }
}

fn next_token<'a>(bytes: &'a [u8], idx: &mut usize) -> Option<&'a str> {
    while *idx < bytes.len() {
        let b = bytes[*idx];
        if b == b'#' {
            while *idx < bytes.len() && bytes[*idx] != b'\n' {
                *idx += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() {
            *idx += 1;
            continue;
        }
        break;
    }
    let start = *idx;
    while *idx < bytes.len() && !bytes[*idx].is_ascii_whitespace() {
        *idx += 1;
    }
    std::str::from_utf8(bytes.get(start..*idx)?).ok()
}

impl Default for SvgImageComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn scale_max_basic() {
        assert_eq!(scale_max(10, 255), 10);
        assert_eq!(scale_max(128, 200), ((128u32 * 255) / 200) as u8);
    }

    #[test]
    fn decode_pnm_p5_and_p6() {
        // P5: 2x1 luma
        let p5 = b"P5\n2 1\n255\n\x05\x06";
        match decode_pnm(p5).unwrap() {
            Pnm::Luma {
                width,
                height,
                data,
            } => {
                assert_eq!(width, 2);
                assert_eq!(height, 1);
                assert_eq!(data, vec![5u8, 6u8]);
            }
            _ => panic!("expected luma"),
        }

        // P6: 2x1 RGB -> expect RGBA with alpha 255
        let p6 = b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06";
        match decode_pnm(p6).unwrap() {
            Pnm::Rgba {
                width,
                height,
                data,
            } => {
                assert_eq!(width, 2);
                assert_eq!(height, 1);
                assert_eq!(data.len(), 8);
                assert_eq!(&data[0..4], &[1u8, 2u8, 3u8, 255u8]);
                assert_eq!(&data[4..8], &[4u8, 5u8, 6u8, 255u8]);
            }
            _ => panic!("expected rgba"),
        }
    }

    #[test]
    fn next_token_handles_comments_and_whitespace() {
        let bytes = b"# this is a comment\nP5 2 1 255\n";
        let mut idx = 0usize;
        // skip comment
        let tok1 = next_token(bytes, &mut idx).unwrap();
        assert_eq!(tok1, "P5");
        let tok2 = next_token(bytes, &mut idx).unwrap();
        assert_eq!(tok2, "2");
    }

    #[test]
    fn load_from_path_accepts_pnm_files() {
        // P5
        let mut f = NamedTempFile::new().expect("create temp file");
        f.write_all(b"P5\n2 1\n255\n\x07\x08").expect("write p5");
        let mut comp = SvgImageComponent::new();
        comp.load_from_path(f.path())
            .expect("load p5 should succeed");

        // P6
        let mut f2 = NamedTempFile::new().expect("create temp file");
        f2.write_all(b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06")
            .expect("write p6");
        comp.load_from_path(f2.path())
            .expect("load p6 should succeed");
    }
}
