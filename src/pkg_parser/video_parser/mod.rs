//! Video support: container detection, pluggable decoding backends and a
//! simple frame scheduler.
//!
//! The module is split in two halves:
//!
//! * the *core* ([`backend`], [`info`], [`player`], [`bulk`]) is plain Rust
//!   and does not depend on any native library, so it can be compiled for
//!   `wasm32-unknown-unknown` and run in a browser;
//! * the *decoders* ([`GifBackend`], [`ImageBackend`], [`FfmpegBackend`])
//!   implement the [`FrameDecoder`] trait and are plugged into the core.
//!
//! The ffmpeg based decoder is only compiled when the `ffmpeg` cargo feature
//! is enabled (it is on by default) and only for the native targets: the
//! native libraries cannot be cross compiled.  It is the only place in the
//! crate that is allowed to mention `ffmpeg_next`.
//!
//! In a browser build the decoders are the ones of the browser itself:
//! the [`WebVideoBackend`] drives the media element of the page (which
//! demuxes *and* decodes the file) and the [`WebImageBackend`] uses the
//! WebCodecs `ImageDecoder`.  They are only compiled for
//! `wasm32-unknown-unknown`.

pub mod backend;
pub mod bulk;
pub mod info;
pub mod mp4_meta;
pub mod player;

// The ffmpeg backend is only compiled for the native targets: the native
// libraries cannot be cross compiled to `wasm32-unknown-unknown`, where
// the browser decodes the videos itself.
#[cfg(all(feature = "ffmpeg", not(target_arch = "wasm32")))]
mod ffmpeg;
mod gif;
// The module is named `still_image` and not `image`: a module called
// `image` would shadow the `image` crate inside this file, and every
// `image::…` path would silently resolve to the module instead.
mod still_image;
#[cfg(target_arch = "wasm32")]
mod web;

use std::io::Cursor;
use std::sync::Arc;

use image::{ImageBuffer, Rgba};

pub use backend::{BackendKind, FrameDecoder, VideoBackend, detect_format};
pub use bulk::{
    DEFAULT_MEMORY_BUDGET, DecodedClip, MEMORY_BUDGET_ENV, decode_all, decode_all_with_backend,
    decode_all_with_budget, estimate_decoded_size, memory_budget, reset_memory_budget,
    set_memory_budget,
};
#[cfg(all(feature = "ffmpeg", not(target_arch = "wasm32")))]
pub use ffmpeg::FfmpegBackend;
pub use gif::GifBackend;
pub use info::{FrameStatus, VideoError, VideoFormat, VideoFrame, VideoInfo};
pub use mp4_meta::{Mp4Meta, probe_mp4};
pub use player::VideoPlayer;
pub use still_image::{ImageBackend, ImageFormat, detect_image_format};
#[cfg(target_arch = "wasm32")]
pub use web::{WebImageBackend, WebVideoBackend};

/// Pick the backend that can decode `bytes`, based on the container/format.
///
/// * an animated GIF is decoded by the pure Rust GIF backend,
/// * a still image (png, jpeg, webp, bmp) by the pure Rust image backend,
/// * an mp4/webm container by the ffmpeg backend — or, in a browser build,
///   by the media element of the page.
///
/// The mp4/webm containers need the `ffmpeg` cargo feature on the native
/// targets: without it the function reports a [`VideoError::Unsupported`].
pub fn backend_for(bytes: &[u8]) -> Result<Box<dyn FrameDecoder>, VideoError> {
    match detect_format(bytes) {
        Some(VideoFormat::Gif) => Ok(Box::new(GifBackend::new(Arc::from(bytes)))),
        Some(format) => match &format {
            VideoFormat::Mp4 | VideoFormat::WebM => video_backend(bytes, mime_for(&format)),
            _ => Err(VideoError::Unsupported(
                "the container format is not supported".to_string(),
            )),
        },
        None => {
            if detect_image_format(bytes).is_some() {
                Ok(Box::new(ImageBackend::new(Arc::from(bytes))))
            } else {
                Err(VideoError::Unsupported(
                    "the data does not look like a video container or a still image".to_string(),
                ))
            }
        }
    }
}

/// The mime type of a video container, as the browser expects it.
fn mime_for(format: &VideoFormat) -> &'static str {
    match format {
        VideoFormat::Mp4 => "video/mp4",
        VideoFormat::WebM => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Create the backend that decodes an mp4/webm container.
///
/// In a browser the media element of the page does the work: it demuxes
/// *and* decodes the file, which is exactly what the WebCodecs
/// `VideoDecoder` cannot do (the codec API only decodes encoded chunks,
/// it knows nothing about the containers).  On the native targets the
/// ffmpeg backend is used instead.
#[cfg(target_arch = "wasm32")]
fn video_backend(bytes: &[u8], mime: &str) -> Result<Box<dyn FrameDecoder>, VideoError> {
    Ok(Box::new(web::WebVideoBackend::new(Arc::from(bytes), mime)))
}

/// Create the backend that decodes an mp4/webm container.
///
/// On the native targets the ffmpeg backend is used; see the browser
/// build for the other case.
#[cfg(not(target_arch = "wasm32"))]
fn video_backend(bytes: &[u8], _mime: &str) -> Result<Box<dyn FrameDecoder>, VideoError> {
    ffmpeg_backend(Arc::from(bytes))
}

/// Create one of the browser backends.
///
/// The browser backends only exist in a browser build: on the other
/// targets the caller gets a [`VideoError::Unsupported`].
#[cfg(target_arch = "wasm32")]
pub(crate) fn web_backend(
    kind: BackendKind,
    bytes: &[u8],
) -> Result<Box<dyn FrameDecoder>, VideoError> {
    match kind {
        BackendKind::WebVideo => {
            let format = detect_format(bytes).unwrap_or(VideoFormat::Mp4);
            Ok(Box::new(web::WebVideoBackend::new(
                Arc::from(bytes),
                mime_for(&format),
            )))
        }
        BackendKind::WebImage => {
            let format = detect_image_format(bytes).unwrap_or(ImageFormat::Png);
            Ok(Box::new(web::WebImageBackend::new(
                Arc::from(bytes),
                format.mime_type(),
            )))
        }
        _ => Err(VideoError::Unsupported(
            "this is not a browser backend".to_string(),
        )),
    }
}

/// Create one of the browser backends.
///
/// The browser backends only exist in a browser build: on the other
/// targets the caller gets a [`VideoError::Unsupported`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn web_backend(
    _kind: BackendKind,
    _bytes: &[u8],
) -> Result<Box<dyn FrameDecoder>, VideoError> {
    Err(VideoError::Unsupported(
        "the browser backends are only available in a browser build".to_string(),
    ))
}

/// Create the ffmpeg backend.
///
/// Without the `ffmpeg` cargo feature the crate has no native decoder at all,
/// so the caller gets an [`VideoError::Unsupported`] instead.
#[cfg(all(feature = "ffmpeg", not(target_arch = "wasm32")))]
pub(crate) fn ffmpeg_backend(bytes: Arc<[u8]>) -> Result<Box<dyn FrameDecoder>, VideoError> {
    Ok(Box::new(ffmpeg::FfmpegBackend::new(bytes)))
}

/// Create the ffmpeg backend.
///
/// Without the `ffmpeg` cargo feature the crate has no native decoder at all,
/// so the caller gets an [`VideoError::Unsupported`] instead.
#[cfg(any(not(feature = "ffmpeg"), target_arch = "wasm32"))]
pub(crate) fn ffmpeg_backend(_bytes: Arc<[u8]>) -> Result<Box<dyn FrameDecoder>, VideoError> {
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
