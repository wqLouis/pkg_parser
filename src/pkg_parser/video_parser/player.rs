//! Backend agnostic frame scheduler.
//!
//! The player owns a [`FrameDecoder`], maps a wall-clock time to a decoded
//! frame and keeps a small cache of the most recently decoded frames so that
//! the same frame can be served repeatedly without decoding it again.
//!
//! On the native targets the decoder runs on a worker thread (see the
//! [`worker`] module): the decoding of a frame then does not block the
//! thread that plays the clip back.  The public API of the player does not
//! change: the frames are asked for by their time, exactly as before.

use std::collections::VecDeque;
use std::sync::Arc;

use log::debug;

use crate::pkg_parser::video_parser::backend::{BackendKind, FrameDecoder, detect_format};
use crate::pkg_parser::video_parser::gif::GifBackend;
use crate::pkg_parser::video_parser::info::{FrameStatus, VideoError, VideoFrame, VideoInfo};
use crate::pkg_parser::video_parser::still_image::{ImageBackend, detect_image_format};

/// The worker thread that decodes the frames.
///
/// There are no threads in a browser build, and the browser decodes the
/// videos asynchronously anyway: the worker only exists on the native
/// targets.
#[cfg(not(target_arch = "wasm32"))]
use crate::pkg_parser::video_parser::worker::Worker;

/// Number of decoded frames kept around by default.
const DEFAULT_CACHE_SIZE: usize = 4;

/// Tolerance used when comparing timestamps (seconds).
///
/// Timestamps come from different sources (container time bases, GIF delays)
/// and are compared with floating point arithmetic, so an epsilon keeps the
/// scheduler from rewinding for a difference of one ULP.
const TIME_EPSILON: f64 = 1e-6;

/// The decoder of the player.
///
/// The decoder is either called directly by the player, or it lives on a
/// worker thread that does the decoding.  The worker keeps the render loop
/// from waiting for the decoding of a frame.
enum Decoder {
    /// The decoder is called directly by the player.
    ///
    /// This is the case in a browser build (there are no threads there,
    /// and the browser decodes the videos asynchronously) and for the
    /// backends that the caller hands over to the player.
    Inline(Box<dyn FrameDecoder>),
    /// The decoder lives on a worker thread.
    #[cfg(not(target_arch = "wasm32"))]
    Worker(Worker),
}

impl Decoder {
    /// The name of the backend.
    fn name(&self) -> &'static str {
        match self {
            Self::Inline(backend) => backend.name(),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Worker(worker) => worker.name(),
        }
    }

    /// Decode the next frame of the stream.
    ///
    /// The horizon is the time up to which the player may need frames: the
    /// worker decodes ahead of it, so that the frames are ready when they
    /// are asked for.
    fn next_frame(&mut self, _horizon: f64) -> Result<FrameStatus, VideoError> {
        match self {
            Self::Inline(backend) => backend.next_frame(),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Worker(worker) => worker.next_frame(_horizon),
        }
    }

    /// Restart decoding from the beginning of the stream.
    fn rewind(&mut self) -> Result<(), VideoError> {
        match self {
            Self::Inline(backend) => backend.rewind(),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Worker(worker) => worker.rewind(),
        }
    }

    /// Ask the decoder for frames of another size.
    fn set_output_size(&mut self, width: u32, height: u32) {
        match self {
            Self::Inline(backend) => backend.set_output_size(width, height),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Worker(worker) => worker.set_output_size(width, height),
        }
    }

    /// Ask the decoder to skip the frames that are never shown.
    fn set_frame_skipping(&mut self, enabled: bool) {
        match self {
            Self::Inline(backend) => backend.set_frame_skipping(enabled),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Worker(worker) => worker.set_frame_skipping(enabled),
        }
    }
}

/// Backend-agnostic frame scheduler: maps a wall-clock time to a decoded frame,
/// caches recently decoded frames, and loops at the end of the stream.
pub struct VideoPlayer {
    /// The decoder doing the actual work.
    decoder: Decoder,
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
    /// The rate at which the clip is played back, in frames per second.
    ///
    /// Zero means "unknown": the frames are then decoded when they are
    /// asked for.
    target_fps: f64,
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
    ///
    /// The backend is used on the calling thread: use
    /// [`open_with_worker_backend`](Self::open_with_worker_backend) to have
    /// it decode on a worker thread instead.
    pub fn open_with_backend(
        bytes: &[u8],
        backend: Box<dyn FrameDecoder>,
    ) -> Result<Self, VideoError> {
        let mut backend = backend;
        let info = backend.probe()?;
        Ok(Self::new(bytes, Decoder::Inline(backend), info))
    }

    /// Open `bytes` with a caller-supplied backend that decodes on a worker
    /// thread.
    ///
    /// The backend is moved to a worker thread that does the decoding: the
    /// thread that plays the clip back does not have to wait for the
    /// decoding of a frame.  The backend has to be [`Send`], because it is
    /// moved to another thread.
    ///
    /// The worker is shut down when the player is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_with_worker_backend(
        bytes: &[u8],
        backend: Box<dyn FrameDecoder + Send>,
    ) -> Result<Self, VideoError> {
        let mut backend = backend;
        let info = backend.probe()?;
        Ok(Self::new(
            bytes,
            Decoder::Worker(Worker::new(backend)),
            info,
        ))
    }

    /// Open `bytes` with an explicitly chosen backend kind.
    ///
    /// The data does not have to be a video container: a still image is
    /// opened with the [`BackendKind::Image`] backend.
    pub fn open_with_backend_kind(bytes: &[u8], kind: BackendKind) -> Result<Self, VideoError> {
        Self::open_with_kind(bytes, kind)
    }

    /// Open `bytes` with an explicitly chosen backend kind, on the native
    /// targets.
    ///
    /// The native backends can be moved to a worker thread: the decoding
    /// then does not block the thread that plays the clip back.
    #[cfg(not(target_arch = "wasm32"))]
    fn open_with_kind(bytes: &[u8], kind: BackendKind) -> Result<Self, VideoError> {
        let backend: Box<dyn FrameDecoder + Send> = match kind {
            BackendKind::Auto => super::sendable_backend_for(bytes)?,
            BackendKind::Gif => {
                if detect_image_format(bytes).is_some() {
                    return Err(VideoError::Unsupported(
                        "the data is a still image, not an animated GIF".to_string(),
                    ));
                }
                Box::new(GifBackend::new(Arc::from(bytes)))
            }
            BackendKind::Ffmpeg => super::sendable_ffmpeg_backend(Arc::from(bytes))?,
            BackendKind::Image => {
                if detect_image_format(bytes).is_none() {
                    debug!("the data does not match a known image format");
                }
                Box::new(ImageBackend::new(Arc::from(bytes)))
            }
            BackendKind::WebVideo | BackendKind::WebImage => {
                return Err(VideoError::Unsupported(
                    "this is not a browser backend".to_string(),
                ));
            }
        };
        Self::open_with_worker_backend(bytes, backend)
    }

    /// Open `bytes` with an explicitly chosen backend kind, in a browser
    /// build.
    ///
    /// A browser build has no threads, and the browser decodes the videos
    /// itself: the decoder stays on the calling thread.
    #[cfg(target_arch = "wasm32")]
    fn open_with_kind(bytes: &[u8], kind: BackendKind) -> Result<Self, VideoError> {
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
            BackendKind::WebVideo | BackendKind::WebImage => super::web_backend(kind, bytes)?,
        };
        Self::open_with_backend(bytes, backend)
    }

    /// Build the player around a decoder.
    fn new(bytes: &[u8], decoder: Decoder, info: VideoInfo) -> Self {
        let format = detect_format(bytes);
        let image_format = detect_image_format(bytes);
        debug!(
            "opening a video with the `{}` backend (container: {:?}, image: {:?})",
            decoder.name(),
            format,
            image_format
        );

        Self {
            decoder,
            info,
            cache: VecDeque::with_capacity(DEFAULT_CACHE_SIZE),
            cache_size: DEFAULT_CACHE_SIZE,
            position: 0.0,
            exhausted: false,
            target_fps: 0.0,
        }
    }

    /// Container metadata (probed once at open time).
    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Name of the backend in use.
    pub fn backend_name(&self) -> &'static str {
        self.decoder.name()
    }

    /// Request frames at this size (optional hint; must be called before the first frame).
    pub fn set_output_size(&mut self, width: u32, height: u32) {
        self.decoder.set_output_size(width, height);
    }

    /// Ask the decoder to skip the frames that will never be shown.
    ///
    /// A clip that is recorded at 60 fps and shown at 30 fps holds half of
    /// its frames for nothing: they are decoded and thrown away right
    /// after.  When the skipping is on, the decoder does not decode them at
    /// all (the ffmpeg backend does not reconstruct the frames that are not
    /// reference frames).
    ///
    /// The frames that are skipped do not exist any more: the player still
    /// hands out the frame that covers the time it is asked for.
    pub fn set_skip_unused_frames(&mut self, enabled: bool) {
        self.decoder.set_frame_skipping(enabled);
    }

    /// Tell the player at which rate the clip is played back.
    ///
    /// The rate is the number of frames per second the clip is shown at
    /// (the frame rate of the compositor, usually).  The player uses it to
    /// decode the frames ahead of the playback, so that the decoding does
    /// not block the renderer.
    ///
    /// A rate of `0.0` (the default) means "unknown": the frames are then
    /// decoded when they are asked for.
    ///
    /// A rate that is lower than the frame rate of the clip also means
    /// that some of the frames are never shown:
    /// [`set_skip_unused_frames`](Self::set_skip_unused_frames) then keeps
    /// the decoder from decoding them.
    pub fn set_target_frame_rate(&mut self, fps: f64) {
        self.target_fps = fps;
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

        // The frames up to the horizon are decoded ahead of the playback:
        // the decoder is asked for them while the player waits for the
        // frames it needs now.
        let horizon = self.horizon(target);

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

            match self.decoder.next_frame(horizon)? {
                FrameStatus::Frame(frame) => {
                    self.push(frame);
                }
                // The decoder is still working on the frame: there is
                // nothing new to show yet.
                FrameStatus::Pending => {
                    break;
                }
                FrameStatus::End => {
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
        self.decoder.rewind()?;
        self.cache.clear();
        self.position = 0.0;
        self.exhausted = false;
        Ok(())
    }

    /// The time up to which the frames are decoded ahead of the playback.
    ///
    /// The player asks for the frames up to the time it needs, plus the
    /// interval between two frames: the worker decodes them while the
    /// player is busy with something else.
    fn horizon(&self, target: f64) -> f64 {
        target + self.lookahead()
    }

    /// The time the frames are decoded ahead of the playback.
    ///
    /// It is the interval between two frames of the playback: the rate of
    /// the playback when it is known, the frame rate of the clip
    /// otherwise.
    fn lookahead(&self) -> f64 {
        if self.target_fps > 0.0 {
            1.0 / self.target_fps
        } else if self.info.fps > 0.0 {
            1.0 / self.info.fps
        } else {
            0.0
        }
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
