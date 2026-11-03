//! Video support: container detection, pluggable decoding backends and a
//! simple frame scheduler.
//!
//! The module is split in two halves:
//!
//! * the *core* ([`backend`], [`info`], [`player`]) is plain Rust and does
//!   not depend on any native library, so it can be compiled for
//!   `wasm32-unknown-unknown` and run in a browser;
//! * the *decoders* ([`GifBackend`], [`FfmpegBackend`]) implement the
//!   [`VideoBackend`] trait and are plugged into the core.
//!
//! The ffmpeg based decoder is only compiled when the `ffmpeg` cargo feature
//! is enabled (it is on by default); it is the only place in the crate that
//! is allowed to mention `ffmpeg_next`.

pub mod backend;
pub mod info;
pub mod player;

#[cfg(feature = "ffmpeg")]
mod ffmpeg;
mod gif;

use std::io::Cursor;

use image::{ImageBuffer, Rgba};

pub use backend::{BackendKind, VideoBackend, detect_format};
#[cfg(feature = "ffmpeg")]
pub use ffmpeg::FfmpegBackend;
pub use gif::GifBackend;
pub use info::{VideoError, VideoFormat, VideoFrame, VideoInfo};
pub use player::VideoPlayer;

/// Create the ffmpeg backend.
///
/// Without the `ffmpeg` cargo feature the crate has no native decoder at all,
/// so the caller gets an [`VideoError::Unsupported`] instead.
#[cfg(feature = "ffmpeg")]
pub(crate) fn ffmpeg_backend(
    bytes: std::sync::Arc<[u8]>,
) -> Result<Box<dyn VideoBackend>, VideoError> {
    Ok(Box::new(ffmpeg::FfmpegBackend::new(bytes)))
}

/// Create the ffmpeg backend.
///
/// Without the `ffmpeg` cargo feature the crate has no native decoder at all,
/// so the caller gets an [`VideoError::Unsupported`] instead.
#[cfg(not(feature = "ffmpeg"))]
pub(crate) fn ffmpeg_backend(
    _bytes: std::sync::Arc<[u8]>,
) -> Result<Box<dyn VideoBackend>, VideoError> {
    Err(VideoError::Unsupported(
        "this build of pkg_parser has no ffmpeg support; rebuild it with the `ffmpeg` feature"
            .to_string(),
    ))
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
            VideoFormat::Gif => gif::gif_info(bytes),
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
            VideoFormat::Gif => gif::gif_frame(&self.data, index),
            _ => None,
        }
    }

    /// Extract all frames from a GIF as RGBA pixel buffers.
    pub fn extract_all_frames(&self) -> Option<Vec<Vec<u8>>> {
        match &self.format {
            VideoFormat::Gif => gif::gif_all_frames(&self.data),
            _ => None,
        }
    }
}

/// Save all frames of a GIF as separate PNG files, returning
/// a list of (png_bytes, filename_suffix).
/// The `stem` should be the base file name without extension
/// (e.g. "animation" for "animation.gif").
pub fn save_gif_frames(bytes: &[u8], stem: &str) -> Option<Vec<(Vec<u8>, String)>> {
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

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
