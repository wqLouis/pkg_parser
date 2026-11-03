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

impl VideoInfo {
    /// Whether the frames of the clip are delivered as planar YUV 4:2:0.
    ///
    /// A YUV frame takes 1.5 bytes per pixel in memory, an RGBA frame 4:
    /// the memory budget of the eager decoding uses the difference.
    pub fn is_yuv420(&self) -> bool {
        is_yuv420_format(&self.pixel_format)
    }
}

/// Whether a pixel format name is a planar YUV 4:2:0 format.
///
/// The name is the one the ffmpeg descriptors use (`yuv420p`,
/// `yuvj420p`, …).
pub fn is_yuv420_format(pixel_format: &str) -> bool {
    pixel_format.starts_with("yuv420") || pixel_format.starts_with("yuvj420")
}

/// The colour matrix of a video stream.
///
/// The matrix tells how the three YUV planes are combined into the red,
/// green and blue channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    /// ITU-R BT.601: the matrix of the standard definition television.
    Bt601,
    /// ITU-R BT.709: the matrix of the high definition television.
    Bt709,
}

/// The range of the luma and chroma values of a video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvRange {
    /// The limited (studio) range: the luma goes from 16 to 235 and the
    /// chroma from 16 to 240.
    Limited,
    /// The full range: the luma goes from 0 to 255 and the chroma from 0
    /// to 255.
    Full,
}

/// The colour space of a YUV frame.
///
/// The colour space is the pair of a colour matrix and a range: it is
/// everything that is needed to turn the planes of a frame into RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvColorspace {
    /// BT.601, limited range: the usual colour space of the standard
    /// definition content.
    Bt601Limited,
    /// BT.601, full range.
    Bt601Full,
    /// BT.709, limited range: the usual colour space of the high
    /// definition content.
    Bt709Limited,
    /// BT.709, full range.
    Bt709Full,
}

impl YuvColorspace {
    /// The colour space of a stream that reports the given matrix and
    /// range.
    pub fn new(matrix: YuvMatrix, range: YuvRange) -> Self {
        match (matrix, range) {
            (YuvMatrix::Bt601, YuvRange::Limited) => YuvColorspace::Bt601Limited,
            (YuvMatrix::Bt601, YuvRange::Full) => YuvColorspace::Bt601Full,
            (YuvMatrix::Bt709, YuvRange::Limited) => YuvColorspace::Bt709Limited,
            (YuvMatrix::Bt709, YuvRange::Full) => YuvColorspace::Bt709Full,
        }
    }

    /// The colour matrix of the colour space.
    pub fn matrix(&self) -> YuvMatrix {
        match self {
            YuvColorspace::Bt601Limited | YuvColorspace::Bt601Full => YuvMatrix::Bt601,
            YuvColorspace::Bt709Limited | YuvColorspace::Bt709Full => YuvMatrix::Bt709,
        }
    }

    /// The range of the colour space.
    pub fn range(&self) -> YuvRange {
        match self {
            YuvColorspace::Bt601Limited | YuvColorspace::Bt709Limited => YuvRange::Limited,
            YuvColorspace::Bt601Full | YuvColorspace::Bt709Full => YuvRange::Full,
        }
    }

    /// The coefficients of the YUV to RGB conversion of the colour space.
    ///
    /// The conversion is the one of the standard: the luma is scaled and
    /// offset, and the two chroma planes are added with their own
    /// coefficient.  The result is the *gamma encoded* (display) RGB, the
    /// same values a `swscale` conversion produces.
    pub fn coefficients(&self) -> YuvCoefficients {
        let (kr, kb) = match self.matrix() {
            YuvMatrix::Bt601 => (0.299_f32, 0.114_f32),
            YuvMatrix::Bt709 => (0.2126_f32, 0.0722_f32),
        };
        let kg = 1.0 - kr - kb;

        // The chroma is centred on 128 and the luma on 16 (limited range)
        // or on 0 (full range).
        let (y_scale, y_offset, chroma_scale) = match self.range() {
            YuvRange::Limited => (255.0 / 219.0, 16.0, 255.0 / 224.0),
            YuvRange::Full => (1.0, 0.0, 1.0),
        };

        YuvCoefficients {
            y_scale,
            y_offset,
            u_offset: 128.0,
            v_offset: 128.0,
            crv: chroma_scale * 2.0 * (1.0 - kr),
            cgu: -chroma_scale * 2.0 * kb * (1.0 - kb) / kg,
            cgv: -chroma_scale * 2.0 * kr * (1.0 - kr) / kg,
            cbu: chroma_scale * 2.0 * (1.0 - kb),
        }
    }

    /// A short, stable name of the colour space (e.g. `"bt709-limited"`).
    pub fn name(&self) -> &'static str {
        match self {
            YuvColorspace::Bt601Limited => "bt601-limited",
            YuvColorspace::Bt601Full => "bt601-full",
            YuvColorspace::Bt709Limited => "bt709-limited",
            YuvColorspace::Bt709Full => "bt709-full",
        }
    }
}

/// The coefficients of the YUV to RGB conversion of a colour space.
///
/// The conversion of one pixel is
///
/// ```text
/// luma = y_scale * (y - y_offset)
/// red   = luma + crv * (v - v_offset)
/// green = luma + cgu * (u - u_offset) + cgv * (v - v_offset)
/// blue  = luma + cbu * (u - u_offset)
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YuvCoefficients {
    /// The scale of the luma (255/219 for the limited range).
    pub y_scale: f32,
    /// The offset of the luma (16 for the limited range, 0 for the full one).
    pub y_offset: f32,
    /// The offset of the U plane (always 128).
    pub u_offset: f32,
    /// The offset of the V plane (always 128).
    pub v_offset: f32,
    /// The coefficient of the V plane in the red channel.
    pub crv: f32,
    /// The coefficient of the U plane in the green channel.
    pub cgu: f32,
    /// The coefficient of the V plane in the green channel.
    pub cgv: f32,
    /// The coefficient of the U plane in the blue channel.
    pub cbu: f32,
}

impl YuvCoefficients {
    /// Convert one YUV triplet into the gamma encoded RGB.
    ///
    /// The three values are in the 0..255 range and the result is not
    /// clamped: the caller decides what to do with the values that are out
    /// of the range.
    pub fn convert(&self, y: f32, u: f32, v: f32) -> [f32; 3] {
        let luma = self.y_scale * (y - self.y_offset);
        [
            luma + self.crv * (v - self.v_offset),
            luma + self.cgu * (u - self.u_offset) + self.cgv * (v - self.v_offset),
            luma + self.cbu * (u - self.u_offset),
        ]
    }
}

/// The pixels of a decoded frame.
///
/// The video decoders produce planar YUV 4:2:0 pixels: the conversion into
/// RGB is left to the consumer, which usually hands the planes over to the
/// GPU.  The GIF, the still image and the browser backends produce RGBA
/// pixels directly.
#[derive(Debug, Clone)]
pub enum FramePixels {
    /// Tightly packed RGBA8 pixels, `width * height * 4` bytes, straight
    /// (non-premultiplied) alpha.
    ///
    /// The pixels are behind an [`Arc`] so cached frames can be handed out
    /// without copying 8 MB per frame.
    Rgba(Arc<[u8]>),
    /// Planar YUV 4:2:0 pixels: the luma plane at the full resolution and
    /// the two chroma planes at half the width and half the height.
    ///
    /// The planes are behind an [`Arc`], like the RGBA pixels.
    Yuv420 {
        /// The luma plane: `width * height` bytes.
        y: Arc<[u8]>,
        /// The blue difference chroma plane: `(width / 2) * (height / 2)`
        /// bytes.
        u: Arc<[u8]>,
        /// The red difference chroma plane: `(width / 2) * (height / 2)`
        /// bytes.
        v: Arc<[u8]>,
        /// The colour space of the source.
        colorspace: YuvColorspace,
    },
}

impl FramePixels {
    /// The number of bytes the pixels take in memory.
    pub fn byte_len(&self) -> usize {
        match self {
            FramePixels::Rgba(pixels) => pixels.len(),
            FramePixels::Yuv420 { y, u, v, .. } => y.len() + u.len() + v.len(),
        }
    }

    /// Whether the pixels are planar YUV 4:2:0.
    pub fn is_yuv420(&self) -> bool {
        matches!(self, FramePixels::Yuv420 { .. })
    }
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
    /// The pixels of the frame, in the format the decoder produced them.
    pub pixels: FramePixels,
}

impl VideoFrame {
    /// The pixels of the frame as tightly packed RGBA8.
    ///
    /// A frame that already holds RGBA pixels is handed out as it is; a
    /// YUV frame is converted on the CPU, which is what the callers that
    /// need real RGBA pixels (the PNG export of the command line tool, for
    /// instance) ask for.
    ///
    /// The conversion is done on every call: the frames are usually
    /// uploaded to the GPU in their native format, which is much cheaper.
    pub fn rgba(&self) -> Result<Arc<[u8]>, VideoError> {
        match &self.pixels {
            FramePixels::Rgba(pixels) => Ok(Arc::clone(pixels)),
            FramePixels::Yuv420 {
                y,
                u,
                v,
                colorspace,
            } => {
                let rgba = crate::pkg_parser::video_parser::yuv::yuv420_to_rgba(
                    self.width,
                    self.height,
                    y,
                    u,
                    v,
                    *colorspace,
                )?;
                Ok(Arc::from(rgba))
            }
        }
    }

    /// The number of bytes the pixels of the frame take in memory.
    pub fn pixel_bytes(&self) -> usize {
        self.pixels.byte_len()
    }

    /// Whether the frame holds planar YUV 4:2:0 pixels.
    pub fn is_yuv420(&self) -> bool {
        self.pixels.is_yuv420()
    }
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
    /// The clip does not fit into the memory budget of the eager decoding.
    ///
    /// The clip was *not* decoded: the caller can stream it instead (see
    /// the [`bulk`](crate::pkg_parser::video_parser::bulk) module).
    BudgetExceeded(String),
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VideoError::Unsupported(message) => write!(f, "unsupported video: {message}"),
            VideoError::Decode(message) => write!(f, "video decoding failed: {message}"),
            VideoError::Io(message) => write!(f, "video I/O failed: {message}"),
            VideoError::BudgetExceeded(message) => {
                write!(
                    f,
                    "the video does not fit into the memory budget: {message}"
                )
            }
        }
    }
}

impl std::error::Error for VideoError {}
