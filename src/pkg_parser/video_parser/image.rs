//! Pure Rust still image backend.
//!
//! A still image is a stream with exactly one frame, which is what lets a
//! browser build implement both WebCodecs decoders — `VideoDecoder` for the
//! videos and `ImageDecoder` for the images — behind the single
//! [`FrameDecoder`] trait.
//!
//! The decoding is done by the `image` crate: no native dependency at all,
//! so the backend works in the browser (wasm) build too.

use std::io::Cursor;
use std::sync::Arc;

use image::RgbaImage;
use image::imageops::FilterType;
use log::debug;

use crate::pkg_parser::video_parser::backend::FrameDecoder;
use crate::pkg_parser::video_parser::info::{VideoError, VideoFrame, VideoInfo};

/// The still image formats the backend recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Portable Network Graphics (`.png`).
    Png,
    /// JPEG (`.jpg`, `.jpeg`).
    Jpeg,
    /// WebP (`.webp`).
    WebP,
    /// Windows bitmap (`.bmp`).
    Bmp,
}

impl ImageFormat {
    /// A short, stable name of the format (e.g. `"png"`).
    pub fn name(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::WebP => "webp",
            ImageFormat::Bmp => "bmp",
        }
    }
}

/// Detect the still image format from the magic bytes.
///
/// * PNG: the 8 byte `\x89PNG\r\n\x1a\n` signature.
/// * JPEG: the `FFD8FF` start of image marker.
/// * WebP: a `RIFF` container with a `WEBP` tag.
/// * BMP: the `BM` signature.
///
/// Returns `None` when the bytes do not match any known image format.
pub fn detect_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    // PNG: the signature is fixed and cannot be forged accidentally.
    if bytes.len() >= 8 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some(ImageFormat::Png);
    }
    // JPEG: `FF D8` is the start of image marker, `FF` the marker prefix.
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(ImageFormat::Jpeg);
    }
    // WebP: a RIFF container whose payload is tagged `WEBP`.
    if bytes.len() >= 12 && bytes[..4] == *b"RIFF" && bytes[8..12] == *b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    // BMP: the bitmap file header starts with `BM`.
    if bytes.len() >= 2 && bytes[..2] == *b"BM" {
        return Some(ImageFormat::Bmp);
    }
    None
}

/// Pure-Rust still image backend built on the `image` crate (no ffmpeg,
/// wasm-friendly).
///
/// The backend keeps the raw bytes around so that it can decode the image
/// again after a [`rewind`](FrameDecoder::rewind).
///
/// A still image is a stream with exactly one frame: the frame is handed out
/// once and the backend then reports the end of the stream.
pub struct ImageBackend {
    /// The raw image payload.
    bytes: Arc<[u8]>,
    /// The decoded image, at its native resolution.
    image: Option<RgbaImage>,
    /// The size the frames have to be delivered in, when the caller asked
    /// for it.
    output_size: Option<(u32, u32)>,
    /// Whether the single frame was already handed out.
    consumed: bool,
}

impl ImageBackend {
    /// Create a backend for the given image payload.
    pub fn new(bytes: Arc<[u8]>) -> Self {
        Self {
            bytes,
            image: None,
            output_size: None,
            consumed: false,
        }
    }

    /// The format of the image, when it is one of the known ones.
    pub fn format(&self) -> Option<ImageFormat> {
        detect_image_format(&self.bytes)
    }

    /// Read the dimensions of the image out of its header.
    ///
    /// The pixels are not decoded: only the header is read.
    fn dimensions(&self) -> Result<(u32, u32), VideoError> {
        let reader = image::ImageReader::new(Cursor::new(self.bytes.clone()))
            .with_guessed_format()
            .map_err(|error| {
                VideoError::Decode(format!("cannot read the image header: {error}"))
            })?;

        reader
            .into_dimensions()
            .map_err(|error| VideoError::Decode(format!("cannot read the image header: {error}")))
    }

    /// Decode the image, keeping the result around for the next call.
    fn decoded(&mut self) -> Result<&RgbaImage, VideoError> {
        if self.image.is_none() {
            let decoded = image::load_from_memory(&self.bytes)
                .map(|image| image.to_rgba8())
                .map_err(|error| VideoError::Decode(format!("cannot decode the image: {error}")))?;
            self.image = Some(decoded);
        }

        match &self.image {
            Some(image) => Ok(image),
            None => Err(VideoError::Decode(
                "the image could not be decoded".to_string(),
            )),
        }
    }
}

impl FrameDecoder for ImageBackend {
    fn name(&self) -> &'static str {
        "image"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        let (width, height) = self.dimensions()?;
        let codec = self
            .format()
            .map(|format| format.name().to_string())
            .unwrap_or_else(|| "image".to_string());

        Ok(VideoInfo {
            width,
            height,
            // A still image has no duration: it is on screen forever.
            duration: 0.0,
            fps: 0.0,
            frame_count: 1,
            codec,
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, VideoError> {
        // A still image has a single frame: once it was handed out, the
        // stream is over.
        if self.consumed {
            return Ok(None);
        }

        let output_size = self.output_size;
        let image = self.decoded()?;
        let (width, height) = image.dimensions();
        let (out_width, out_height) = output_size.unwrap_or((width, height));

        let rgba: Arc<[u8]> = if out_width == width && out_height == height {
            Arc::from(image.as_raw().as_slice())
        } else {
            // Nearest neighbour is good enough for a still image.
            let scaled = image::imageops::resize(image, out_width, out_height, FilterType::Nearest);
            Arc::from(scaled.into_raw())
        };

        self.consumed = true;

        Ok(Some(VideoFrame {
            width: out_width,
            height: out_height,
            time: 0.0,
            duration: 0.0,
            rgba,
        }))
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        debug!("rewinding the image decoder");
        // The decoded pixels are kept around: the frame can be produced
        // again without decoding the image a second time.
        self.consumed = false;
        Ok(())
    }

    fn set_output_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.output_size = Some((width, height));
    }
}
