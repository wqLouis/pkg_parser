//! Data types shared by every video backend.
//!
//! The types in this module are deliberately free of any decoder specific
//! types: the core of the crate has to stay compilable for
//! `wasm32-unknown-unknown`, where no native decoder exists.  Everything a
//! backend produces is expressed with plain Rust types, so a browser build
//! can plug in a WebCodecs bridge without touching the rest of the crate.

use std::fmt;
use std::sync::Arc;

/// Detected video container format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoFormat {
    /// ISO base media container (`.mp4`, `.mov`, …).
    Mp4,
    /// Matroska container, usually carrying VP8/VP9 video (`.webm`).
    WebM,
    /// Animated GIF.
    Gif,
    /// A container the parser does not know (yet).
    Unknown(String),
}

/// Metadata about a video stream, as reported by the backend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoInfo {
    /// Width of the video in pixels.
    pub width: u32,
    /// Height of the video in pixels.
    pub height: u32,
    /// Length of the stream in seconds; `0.0` when unknown.
    pub duration: f64,
    /// Frames per second; `0.0` when unknown.
    pub fps: f64,
    /// Best estimate of the number of frames in the stream.
    pub frame_count: u32,
    /// Name of the codec, e.g. `"h264"`.
    pub codec: String,
    /// Name of the pixel format, e.g. `"yuv420p"`.
    pub pixel_format: String,
    /// Whether the container carries an audio stream.
    pub has_audio: bool,
}

/// A single decoded frame.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// Width of the frame in pixels.
    pub width: u32,
    /// Height of the frame in pixels.
    pub height: u32,
    /// Presentation time of the frame, in seconds from the start of the stream.
    pub time: f64,
    /// How long this frame is displayed, in seconds (best effort).
    pub duration: f64,
    /// Tightly packed RGBA8 pixels, `width * height * 4` bytes, straight
    /// (non-premultiplied) alpha.
    ///
    /// The pixels are behind an [`Arc`] so cached frames can be handed out
    /// without copying 8 MB per frame.
    pub rgba: Arc<[u8]>,
}

/// The outcome of a call to
/// [`FrameDecoder::next_frame`](crate::pkg_parser::video_parser::backend::FrameDecoder::next_frame).
///
/// A decoder is not necessarily synchronous: the browser decodes a video
/// in the media element of the page and an image with the WebCodecs
/// `ImageDecoder`, and both of them hand the frame over asynchronously.
/// `Pending` tells the caller that the decoder is still working on the
/// frame and that it should ask again later.
#[derive(Debug, Clone)]
pub enum FrameStatus {
    /// A frame was decoded and is ready to be shown.
    Frame(VideoFrame),
    /// The decoder is still working on it: the frame is not ready yet.
    Pending,
    /// The end of the stream was reached.
    End,
}

/// Everything that can go wrong while opening or decoding a video.
#[derive(Debug)]
pub enum VideoError {
    /// No backend can handle this container/format.
    Unsupported(String),
    /// The backend failed while decoding.
    Decode(String),
    /// I/O failure.
    Io(String),
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VideoError::Unsupported(message) => write!(f, "unsupported video: {message}"),
            VideoError::Decode(message) => write!(f, "video decoding failed: {message}"),
            VideoError::Io(message) => write!(f, "video I/O failed: {message}"),
        }
    }
}

impl std::error::Error for VideoError {}
