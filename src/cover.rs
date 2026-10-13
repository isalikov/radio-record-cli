//! Track cover art for the now-playing panel. A background fetch downloads
//! the API's cover, decodes it (JPEG or PNG), and shrinks it once to a small
//! RGB grid; the UI box-samples that grid into cells at the chosen `CoverMode`,
//! which gives the pixel-art look and works in every truecolor terminal. The
//! decoded original is kept for terminals that can show real pictures.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::Duration;

/// How much detail the now-playing cover shows, coarsest first. Saved in
/// `settings.json` as `"cover"`, cycled with `c`, and overridden for one
/// session by `RADIO_RECORD_COVER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoverMode {
    /// No cover: nothing is downloaded and the details take its place.
    Off,
    /// Big square blocks, two cells wide and one row tall.
    Chunky,
    /// Half-blocks: two square pixels per cell.
    #[default]
    Pixel,
    /// Quadrant blocks: four pixels per cell, two colors each.
    Fine,
    /// A real picture through the terminal's graphics protocol, if any.
    Sharp,
}

impl CoverMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Chunky,
        Self::Pixel,
        Self::Fine,
        Self::Sharp,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Chunky => "chunky",
            Self::Pixel => "pixel",
            Self::Fine => "fine",
            Self::Sharp => "sharp",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == value)
    }

    /// The next mode with more detail, wrapping from `sharp` to `off`.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

impl Serialize for CoverMode {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// An unknown name (say, from a newer version) falls back to the default
/// instead of making the whole settings file unreadable.
impl<'de> Deserialize<'de> for CoverMode {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::parse(&name).unwrap_or_default())
    }
}

/// Stored resolution: twice the largest panel cover (14 cells, 14 pixels)
/// so every size the UI draws is a clean downsample.
const STORED: u32 = 28;
const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cover {
    pub url: String,
    pub image: image::RgbImage,
    pixels: Vec<[u8; 3]>,
}

impl Cover {
    /// Download and decode `url` on the calling (background) thread.
    pub fn fetch(url: &str) -> Result<Self> {
        let http: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(crate::api::USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        let response = http
            .get(url)
            .call()
            .map_err(|err| Error::new(format!("cover request failed: {err}")))?;
        let mut bytes = Vec::new();
        response
            .into_body()
            .as_reader()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| Error::new(format!("cover download failed: {err}")))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::new("cover image is too large"));
        }
        Self::decode(url, &bytes)
    }

    pub fn decode(url: &str, bytes: &[u8]) -> Result<Self> {
        let image = image::load_from_memory(bytes)
            .map_err(|err| Error::new(format!("cover image unreadable: {err}")))?
            .to_rgb8();
        let small = image::imageops::thumbnail(&image, STORED, STORED);
        Ok(Self {
            url: url.to_owned(),
            pixels: small.pixels().map(|pixel| pixel.0).collect(),
            image,
        })
    }

    /// The cover as `width × height` pixels (row-major), each the average of
    /// the stored pixels it covers. Rendering two pixels per cell keeps them square.
    pub fn sample(&self, width: u16, height: u16) -> Vec<[u8; 3]> {
        let (width, height) = (usize::from(width), usize::from(height));
        let side = STORED as usize;
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            let (top, bottom) = span(y, height, side);
            for x in 0..width {
                let (left, right) = span(x, width, side);
                let mut sum = [0u32; 3];
                for row in top..bottom {
                    for pixel in &self.pixels[row * side + left..row * side + right] {
                        for (total, channel) in sum.iter_mut().zip(pixel) {
                            *total += u32::from(*channel);
                        }
                    }
                }
                let count = ((bottom - top) * (right - left)) as u32;
                out.push(sum.map(|total| (total / count) as u8));
            }
        }
        out
    }
}

/// The stored rows (or columns) that output index `index` of `count` covers.
fn span(index: usize, count: usize, side: usize) -> (usize, usize) {
    let start = index * side / count;
    let end = ((index + 1) * side / count).max(start + 1).min(side);
    (start.min(side - 1), end)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small solid-orange cover for UI and app tests.
    pub(crate) fn cover(url: &str) -> Cover {
        Cover::decode(url, &png(4, 4, |_, _| [250, 120, 30])).unwrap()
    }

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| image::Rgb(pixel(x, y)));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn decodes_and_samples_a_two_tone_cover() {
        // Left half red, right half blue.
        let bytes = png(
            100,
            100,
            |x, _| {
                if x < 50 { [255, 0, 0] } else { [0, 0, 255] }
            },
        );
        let cover = Cover::decode("https://example.com/a.png", &bytes).unwrap();
        assert_eq!(cover.url, "https://example.com/a.png");
        let pixels = cover.sample(4, 2);
        assert_eq!(pixels.len(), 8);
        assert_eq!(pixels[0], [255, 0, 0]);
        assert_eq!(pixels[3], [0, 0, 255]);
        assert_eq!(pixels[4], [255, 0, 0]);
        assert_eq!(pixels[7], [0, 0, 255]);
    }

    #[test]
    fn sampling_survives_every_panel_size() {
        let bytes = png(7, 3, |x, y| [x as u8 * 30, y as u8 * 80, 9]);
        let cover = Cover::decode("u", &bytes).unwrap();
        for (width, height) in [(1, 1), (10, 10), (14, 14), (28, 28), (40, 40)] {
            assert_eq!(
                cover.sample(width, height).len(),
                usize::from(width) * usize::from(height)
            );
        }
        assert!(cover.sample(0, 5).is_empty());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(Cover::decode("u", b"not an image").is_err());
        assert!(Cover::decode("u", &[]).is_err());
    }

    #[test]
    fn cover_modes_cycle_and_round_trip_by_name() {
        let mut mode = CoverMode::default();
        assert_eq!(mode, CoverMode::Pixel);
        let mut seen = Vec::new();
        for _ in 0..CoverMode::ALL.len() {
            mode = mode.next();
            seen.push(mode);
            assert_eq!(CoverMode::parse(mode.name()), Some(mode));
        }
        assert_eq!(mode, CoverMode::Pixel, "the cycle visits every mode once");
        assert!(seen.contains(&CoverMode::Off) && seen.contains(&CoverMode::Sharp));
        assert_eq!(CoverMode::parse("blurry"), None);
    }
}
