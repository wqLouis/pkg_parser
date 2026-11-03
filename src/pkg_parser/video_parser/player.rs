//! Backend agnostic frame scheduler.
//!
//! The player owns a [`FrameDecoder`], maps a wall-clock time to a decoded
//! frame and keeps a small cache of the most recently decoded frames so that
//! the same frame can be served repeatedly without decoding it again.

use std::collections::VecDeque;
use std::sync::Arc;

use log::debug;

use crate::pkg_parser::video_parser::backend::{BackendKind, FrameDecoder, detect_format};
use crate::pkg_parser::video_parser::gif::GifBackend;
use crate::pkg_parser::video_parser::image::{ImageBackend, detect_image_format};
use crate::pkg_parser::video_parser::info::{VideoError, VideoFrame, VideoInfo};

/// Number of decoded frames kept around by default.
const DEFAULT_CACHE_SIZE: usize = 4;

/// Tolerance used when comparing timestamps (seconds).
///
/// Timestamps come from different sources (container time bases, GIF delays)
/// and are compared with floating point arithmetic, so an epsilon keeps the
/// scheduler from rewinding for a difference of one ULP.
const TIME_EPSILON: f64 = 1e-6;

/// Backend-agnostic frame scheduler: maps a wall-clock time to a decoded frame,
/// caches recently decoded frames, and loops at the end of the stream.
pub struct VideoPlayer {
    /// The decoder doing the actual work.
    backend: Box<dyn FrameDecoder>,
    /// Container metadata, probed once when the player was opened.
    info: VideoInfo,
    /// Recently decoded frames, oldest first.
    cache: VecDeque<VideoFrame>,
    /// Maximum number of frames kept in `cache`.
    cache_size: usize,
    /// Timestamp of the frame that is currently on screen.
    position: f64,
    /// Whether the backend signalled the end of the stream.
    exhausted: bool,
}

impl VideoPlayer {
    /// Open `bytes` with the best available backend for its container.
    pub fn open(bytes: &[u8]) -> Result<Self, VideoError> {
        Self::open_with_backend_kind(bytes, BackendKind::Auto)
    }

    /// Open `bytes` with a caller-supplied backend (this is the extension point for
    /// alternative backends, e.g. a WebCodecs bridge in a browser build).
    ///
    /// The backend owns the payload it decodes; `bytes` is only used to
    /// report the container of the data in the logs.
    pub fn open_with_backend(
        bytes: &[u8],
        backend: Box<dyn FrameDecoder>,
    ) -> Result<Self, VideoError> {
        let format = detect_format(bytes);
        let image_format = detect_image_format(bytes);
        debug!(
            "opening a video with the `{}` backend (container: {:?}, image: {:?})",
            backend.name(),
            format,
            image_format
        );

        let mut backend = backend;
        let info = backend.probe()?;
        Ok(Self {
            backend,
            info,
            cache: VecDeque::with_capacity(DEFAULT_CACHE_SIZE),
            cache_size: DEFAULT_CACHE_SIZE,
            position: 0.0,
            exhausted: false,
        })
    }

    /// Open `bytes` with an explicitly chosen backend kind.
    ///
    /// The data does not have to be a video container: a still image is
    /// opened with the [`BackendKind::Image`] backend.
    pub fn open_with_backend_kind(bytes: &[u8], kind: BackendKind) -> Result<Self, VideoError> {
        let backend: Box<dyn FrameDecoder> = match kind {
            BackendKind::Auto => super::backend_for(bytes)?,
            BackendKind::Gif => {
                if detect_image_format(bytes).is_some() {
                    return Err(VideoError::Unsupported(
                        "the data is a still image, not an animated GIF".to_string(),
                    ));
                }
                Box::new(GifBackend::new(Arc::from(bytes)))
            }
            BackendKind::Ffmpeg => super::ffmpeg_backend(Arc::from(bytes))?,
            BackendKind::Image => {
                if detect_image_format(bytes).is_none() {
                    debug!("the data does not match a known image format");
                }
                Box::new(ImageBackend::new(Arc::from(bytes)))
            }
        };

        Self::open_with_backend(bytes, backend)
    }

    /// Container metadata (probed once at open time).
    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Name of the backend in use.
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Request frames at this size (optional hint; must be called before the first frame).
    pub fn set_output_size(&mut self, width: u32, height: u32) {
        self.backend.set_output_size(width, height);
    }

    /// The frame that covers `time_secs` (seconds from the start of the stream).
    ///
    /// Sequential playback is O(1) amortised; a backwards jump rewinds and re-decodes.
    /// Returns `Ok(None)` when the stream is exhausted and nothing is cached.
    pub fn frame_at(&mut self, time_secs: f64) -> Result<Option<VideoFrame>, VideoError> {
        let target = if time_secs.is_nan() {
            0.0
        } else {
            time_secs.max(0.0)
        };

        // A backwards jump cannot be served from the cache: rewind and decode
        // the stream again from its first frame.
        if target + TIME_EPSILON < self.position {
            self.reset()?;
        }

        loop {
            // Decode as long as the newest frame still covers the requested
            // time; the first frame *after* it ends the search.
            let covered = match self.cache.back() {
                Some(frame) => frame.time <= target + TIME_EPSILON,
                None => true,
            };
            if !covered || self.exhausted {
                break;
            }

            match self.backend.next_frame()? {
                Some(frame) => {
                    self.push(frame);
                }
                None => {
                    self.exhausted = true;
                    break;
                }
            }
        }

        let frame = self.pick(target);
        if let Some(frame) = &frame {
            // The playback position follows the frame that is on screen: a
            // request for the same time is then served from the cache.
            self.position = frame.time;
        }
        Ok(frame)
    }

    /// Rewind to the start of the stream (drops the cache).
    pub fn reset(&mut self) -> Result<(), VideoError> {
        self.backend.rewind()?;
        self.cache.clear();
        self.position = 0.0;
        self.exhausted = false;
        Ok(())
    }

    /// Store a freshly decoded frame, dropping the oldest ones when the cache
    /// is full.
    fn push(&mut self, frame: VideoFrame) {
        if self.cache_size == 0 {
            return;
        }
        while self.cache.len() >= self.cache_size {
            self.cache.pop_front();
        }
        self.cache.push_back(frame);
    }

    /// Pick the frame covering `target` out of the cache.
    ///
    /// The covering frame is the newest one whose timestamp is not after
    /// `target`; when `target` is before the first frame the first frame is
    /// returned instead.
    fn pick(&self, target: f64) -> Option<VideoFrame> {
        let mut chosen = None;
        for frame in &self.cache {
            if frame.time <= target + TIME_EPSILON {
                chosen = Some(frame.clone());
            }
        }
        chosen.or_else(|| self.cache.front().cloned())
    }
}
