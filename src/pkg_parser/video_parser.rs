use std::io::Cursor;

use image::{AnimationDecoder, ImageBuffer, ImageDecoder, Rgba, codecs::gif::GifDecoder};

/// Detected video or GIF format.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoFormat {
    Mp4,
    WebM,
    Gif,
    Unknown(String),
}

/// Parsed video or GIF data from a package file.
#[derive(Debug, Clone)]
pub struct Video {
    pub data: Vec<u8>,
    pub format: VideoFormat,
    pub dimensions: Option<(u32, u32)>,
    pub frame_count: Option<u32>,
}

impl Video {
    /// Try to parse raw bytes as a known video or GIF format.
    ///
    /// Returns `None` if the bytes don't match any supported format.
    pub fn new(bytes: &[u8]) -> Option<Video> {
        let format = detect_format(bytes)?;

        let (dimensions, frame_count) = match &format {
            VideoFormat::Gif => extract_gif_info(bytes),
            _ => (None, None),
        };

        Some(Video {
            data: bytes.to_vec(),
            format,
            dimensions,
            frame_count,
        })
    }

    /// Whether this is a video format (mp4, webm, etc.).
    pub fn is_video(&self) -> bool {
        matches!(self.format, VideoFormat::Mp4 | VideoFormat::WebM)
    }

    /// Whether this is an animated GIF.
    pub fn is_gif(&self) -> bool {
        matches!(self.format, VideoFormat::Gif)
    }

    /// Extract a single frame from a GIF as RGBA pixels.
    ///
    /// For video formats (mp4/webm) this returns `None` because
    /// frame decoding would require a heavier dependency (ffmpeg etc.).
    pub fn extract_frame(&self, index: u32) -> Option<Vec<u8>> {
        match &self.format {
            VideoFormat::Gif => extract_gif_frame(&self.data, index),
            _ => None,
        }
    }

    /// Extract all frames from a GIF as RGBA pixel buffers.
    pub fn extract_all_frames(&self) -> Option<Vec<Vec<u8>>> {
        match &self.format {
            VideoFormat::Gif => {
                let decoder = GifDecoder::new(Cursor::new(&self.data)).ok()?;
                let frames = decoder.into_frames().collect_frames().ok()?;
                Some(
                    frames
                        .into_iter()
                        .map(|f| f.into_buffer().into_raw())
                        .collect(),
                )
            }
            _ => None,
        }
    }
}

/// Detect the format from magic bytes.
fn detect_format(bytes: &[u8]) -> Option<VideoFormat> {
    // MP4/QuickTime: starts with ftyp box (size + "ftyp")
    if bytes.len() >= 8 && bytes[4..8] == [0x66, 0x74, 0x79, 0x70] {
        return Some(VideoFormat::Mp4);
    }
    // WebM: starts with EBML header
    if bytes.len() >= 4 && bytes[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(VideoFormat::WebM);
    }
    // GIF87a or GIF89a
    if bytes.len() >= 6
        && bytes[..3] == [0x47, 0x49, 0x46]
        && bytes[3] == 0x38
        && (bytes[4] == 0x37 || bytes[4] == 0x39)
        && bytes[5] == 0x61
    {
        return Some(VideoFormat::Gif);
    }
    None
}

/// Extract basic info (dimensions, frame count) from a GIF.
fn extract_gif_info(bytes: &[u8]) -> (Option<(u32, u32)>, Option<u32>) {
    let decoder = GifDecoder::new(Cursor::new(bytes));
    let decoder = match decoder {
        Ok(d) => d,
        Err(_) => return (None, None),
    };

    let dimensions = match decoder.dimensions() {
        (w, h) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    };

    let frame_count = decoder
        .into_frames()
        .collect_frames()
        .ok()
        .map(|frames| frames.len() as u32);

    (dimensions, frame_count)
}

/// Extract a single frame from a GIF by index.
fn extract_gif_frame(bytes: &[u8], index: u32) -> Option<Vec<u8>> {
    let decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;

    let frame = frames.into_iter().nth(index as usize)?;
    Some(frame.into_buffer().into_raw())
}

/// Save all frames of a GIF as separate PNG files, returning
/// a list of (png_bytes, filename_suffix).
/// The `stem` should be the base file name without extension
/// (e.g. "animation" for "animation.gif").
pub fn save_gif_frames(bytes: &[u8], stem: &str) -> Option<Vec<(Vec<u8>, String)>> {
    let decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;

    let mut results = Vec::with_capacity(frames.len());
    for (i, frame) in frames.into_iter().enumerate() {
        let rgba = frame.into_buffer();
        let (w, h) = rgba.dimensions();

        let mut png_buf = Vec::new();
        let mut cur = Cursor::new(&mut png_buf);
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba.into_raw())?
            .write_to(&mut cur, image::ImageFormat::Png)
            .ok()?;

        results.push((png_buf, format!("{}_frame_{:04}.png", stem, i)));
    }

    Some(results)
}
