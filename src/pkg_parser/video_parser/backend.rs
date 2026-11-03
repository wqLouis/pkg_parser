//! The pluggable decoder interface.
//!
//! Every pixel decoder used by this crate implements [`VideoBackend`].  The
//! trait is intentionally *sequential* (decode the next frame / rewind) so
//! that any implementation — ffmpeg on native targets, a WebCodecs bridge in
//! the browser — can satisfy it.  Random access is implemented by the player
//! on top of it.

use crate::pkg_parser::video_parser::info::{VideoError, VideoFormat, VideoFrame, VideoInfo};

/// A pluggable video decoding backend.
///
/// The trait is intentionally *sequential* (decode the next frame / rewind) so that any
/// implementation — ffmpeg on native targets, a WebCodecs bridge in the browser — can
/// satisfy it. Random access is implemented by the player on top of it.
pub trait VideoBackend {
    /// Short, stable identifier of the backend (e.g. `"ffmpeg"`, `"gif"`).
    fn name(&self) -> &'static str;

    /// Inspect the container: dimensions, duration, fps, codec, …
    fn probe(&mut self) -> Result<VideoInfo, VideoError>;

    /// Decode the next frame in presentation order. `Ok(None)` = end of stream.
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, VideoError>;

    /// Restart decoding from the beginning of the stream.
    fn rewind(&mut self) -> Result<(), VideoError>;

    /// Ask the backend to deliver frames scaled to this size (optional; backends may ignore it).
    /// Called before the first `next_frame`.
    fn set_output_size(&mut self, _width: u32, _height: u32) {}
}

/// Which backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Pick the best backend for the container format.
    Auto,
    /// Force the ffmpeg backend (needs the `ffmpeg` cargo feature).
    Ffmpeg,
    /// Force the pure Rust GIF backend.
    Gif,
}

/// Detect the container format from the magic bytes.
///
/// * MP4/QuickTime: an `ftyp` box at offset 4.
/// * WebM/Matroska: an EBML header (`0x1A 0x45 0xDF 0xA3`).
/// * GIF: the `GIF87a` or `GIF89a` signature.
///
/// Returns `None` when the bytes do not match any known container.
pub fn detect_format(bytes: &[u8]) -> Option<VideoFormat> {
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
