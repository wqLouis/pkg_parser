//! Pure Rust GIF backend.
//!
//! The GIF decoder is built on the `image` crate: it needs no native
//! dependency at all, which makes it the backend of choice for the browser
//! (wasm) build and the fallback when the crate is compiled without the
//! `ffmpeg` feature.

use std::io::Cursor;
use std::sync::Arc;

use image::{AnimationDecoder, ImageDecoder, RgbaImage, codecs::gif::GifDecoder};
use log::debug;

use crate::pkg_parser::video_parser::backend::FrameDecoder;
use crate::pkg_parser::video_parser::info::{
    FramePixels, FrameStatus, VideoError, VideoFrame, VideoInfo,
};

/// Pure-Rust GIF backend built on the `image` crate (no ffmpeg, wasm-friendly).
///
/// The backend keeps the raw bytes around so that it can rebuild its decoder
/// when the stream has to be replayed.
///
/// The frames are always handed out in their native resolution: the backend
/// ignores the output size requested by the player.
pub struct GifBackend {
    /// The raw GIF payload.
    bytes: Arc<[u8]>,
    /// The frame iterator; `None` until the stream is opened.
    frames: Option<Frames>,
    /// Presentation time of the next frame, in seconds.
    time: f64,
}

/// The frame iterator of the GIF decoder.
///
/// The iterator of the `image` crate is not [`Send`] (it holds a trait
/// object), but it is only ever used by the thread that owns the backend:
/// the wrapper makes the backend movable to the worker thread that
/// decodes the frames of a clip.
struct Frames(image::Frames<'static>);

// SAFETY: the frame iterator is only used by the thread that owns the
// backend it belongs to; it is never shared between two threads.
unsafe impl Send for Frames {}

impl GifBackend {
    /// Create a backend for the given GIF payload.
    pub fn new(bytes: Arc<[u8]>) -> Self {
        Self {
            bytes,
            frames: None,
            time: 0.0,
        }
    }

    /// Build a fresh decoder for the stored bytes.
    fn decoder(&self) -> Result<GifDecoder<Cursor<Arc<[u8]>>>, VideoError> {
        GifDecoder::new(Cursor::new(self.bytes.clone()))
            .map_err(|error| VideoError::Decode(format!("cannot read the GIF: {error}")))
    }
}

impl FrameDecoder for GifBackend {
    fn name(&self) -> &'static str {
        "gif"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        let decoder = self.decoder()?;
        let (width, height) = decoder.dimensions();

        // Walk the animation once to count the frames and to sum up their
        // delays (the GIF delays are expressed in 1/100 s).
        let mut frame_count = 0u32;
        let mut duration = 0.0f64;
        for frame in decoder.into_frames() {
            let frame = frame.map_err(|error| {
                VideoError::Decode(format!("cannot decode a GIF frame: {error}"))
            })?;
            duration += delay_seconds(frame.delay());
            frame_count += 1;
        }

        let fps = if duration > 0.0 {
            f64::from(frame_count) / duration
        } else {
            0.0
        };

        Ok(VideoInfo {
            width,
            height,
            duration,
            fps,
            frame_count,
            codec: "gif".to_string(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
        if self.frames.is_none() {
            let decoder = self.decoder()?;
            self.frames = Some(Frames(decoder.into_frames()));
            self.time = 0.0;
        }

        let frames = &mut self
            .frames
            .as_mut()
            .ok_or_else(|| VideoError::Decode("the GIF decoder is not open".to_string()))?
            .0;
        let frame = match frames.next() {
            Some(frame) => frame.map_err(|error| {
                VideoError::Decode(format!("cannot decode a GIF frame: {error}"))
            })?,
            None => return Ok(FrameStatus::End),
        };

        let delay = delay_seconds(frame.delay());
        let buffer: RgbaImage = frame.into_buffer();
        let (width, height) = buffer.dimensions();

        let time = self.time;
        self.time += delay;

        Ok(FrameStatus::Frame(VideoFrame {
            width,
            height,
            time,
            duration: delay,
            pixels: FramePixels::Rgba(Arc::from(buffer.into_raw())),
        }))
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        debug!("rewinding the GIF decoder");
        // Dropping the iterator closes the decoder; the next call to
        // `next_frame` rebuilds it from the stored bytes.
        self.frames = None;
        self.time = 0.0;
        Ok(())
    }
}

/// Convert a GIF delay into seconds.
///
/// The `image` crate expresses the delays in milliseconds; a GIF stores them
/// in 1/100 s, which the decoder already scaled for us.
fn delay_seconds(delay: image::Delay) -> f64 {
    let (numerator, denominator) = delay.numer_denom_ms();
    if denominator == 0 {
        return 0.0;
    }
    (f64::from(numerator) / f64::from(denominator)) / 1000.0
}

/// Extract basic info (dimensions, frame count) from a GIF.
pub(crate) fn gif_info(bytes: &[u8]) -> (Option<(u32, u32)>, Option<u32>) {
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
pub(crate) fn gif_frame(bytes: &[u8], index: u32) -> Option<Vec<u8>> {
    let decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;

    let frame = frames.into_iter().nth(index as usize)?;
    Some(frame.into_buffer().into_raw())
}

/// Extract all frames from a GIF as RGBA pixel buffers.
pub(crate) fn gif_all_frames(bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    let decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    Some(
        frames
            .into_iter()
            .map(|f| f.into_buffer().into_raw())
            .collect(),
    )
}
