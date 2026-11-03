//! Eager decoding of a whole clip into memory.
//!
//! The streaming [`VideoPlayer`](crate::pkg_parser::video_parser::player::VideoPlayer)
//! decodes the frames of a clip while it plays.  That works, but the
//! container is opened again (and the prefix of the clip decoded again)
//! every time the playback loops, and the player only keeps a handful of
//! frames around.  The functions of this module decode a whole clip *once*,
//! in one go: the frames are kept in memory and the caller plays them back
//! by index, without decoding anything again.
//!
//! # Memory budget
//!
//! Decoding a clip in one go costs memory: every frame is a
//! `width * height * 4` byte buffer (a YUV frame is 2.7 times smaller).
//! A *memory budget* guards the eager decoding: the size of the clip is
//! estimated (`width * height * 4 * frame_count`, or
//! `width * height * 3 / 2 * frame_count` for the YUV frames) **before**
//! it is decoded and the decoding is refused when the clip does not fit
//! into the budget.  The caller then falls back to the streaming playback
//! of the
//! [`VideoPlayer`](crate::pkg_parser::video_parser::player::VideoPlayer).
//!
//! The trade-off is deliberate: a clip is decoded **once** and kept in
//! memory for as long as it is played, and the budget is what caps that
//! memory.  A clip that does not fit into the budget is not decoded at
//! all: it is streamed instead, which decodes it again and again.
//!
//! The budget is [`DEFAULT_MEMORY_BUDGET`] by default.  It can be changed
//! with [`set_memory_budget`] (which wins over everything else) or with
//! the `PKG_PARSER_VIDEO_MEMORY_BUDGET` environment variable, which holds
//! the budget in bytes.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use log::debug;

use crate::pkg_parser::video_parser::backend::FrameDecoder;
use crate::pkg_parser::video_parser::info::{FrameStatus, VideoError, VideoFrame, VideoInfo};

/// The default budget for the memory used by an eagerly decoded clip.
///
/// 256 MiB holds the clips a wallpaper usually plays: a 1080p frame is
/// 3.1 MiB (the YUV planes of the decoder) or 8.3 MiB (the RGBA pixels
/// of a converted frame), so the budget covers ~80 of them (about two
/// seconds at 30 fps, more than eight seconds at 10 fps), and a 720p clip
/// of a few seconds.  The clip is decoded once and kept in memory while
/// it plays: the budget is what caps that memory.
pub const DEFAULT_MEMORY_BUDGET: usize = 256 * 1024 * 1024;

/// The name of the environment variable that overrides the default budget.
///
/// The variable holds the budget in bytes (e.g. `67108864` for 64 MiB).
pub const MEMORY_BUDGET_ENV: &str = "PKG_PARSER_VIDEO_MEMORY_BUDGET";

/// The budget of the eager decoding, when it was set explicitly.
static BUDGET: AtomicUsize = AtomicUsize::new(DEFAULT_MEMORY_BUDGET);

/// Whether the budget was set explicitly.
///
/// The environment variable is only consulted when the program did not set
/// the budget itself.
static BUDGET_IS_SET: AtomicBool = AtomicBool::new(false);

/// A clip that was decoded in one go.
///
/// The frames are in presentation order: the caller plays them back by
/// index, which is what the binary search of the frame that covers a
/// given time is based on.
#[derive(Debug, Clone)]
pub struct DecodedClip {
    /// The decoded frames, in presentation order.
    pub frames: Vec<VideoFrame>,
    /// The metadata of the clip.
    ///
    /// The frame count is the number of the frames that were decoded: it
    /// is exact, whereas the one of the probe is an estimate.
    pub info: VideoInfo,
}

impl DecodedClip {
    /// The number of the frames of the clip.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the clip has no frame at all.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The presentation times of the frames, in increasing order.
    pub fn times(&self) -> Vec<f64> {
        self.frames.iter().map(|frame| frame.time).collect()
    }
}

/// The memory budget of the eager decoding, in bytes.
///
/// The budget is [`DEFAULT_MEMORY_BUDGET`], unless it was set explicitly
/// with [`set_memory_budget`] or the [`MEMORY_BUDGET_ENV`] environment
/// variable holds a valid number of bytes.
pub fn memory_budget() -> usize {
    if BUDGET_IS_SET.load(Ordering::Relaxed) {
        return BUDGET.load(Ordering::Relaxed);
    }
    budget_from_env().unwrap_or(DEFAULT_MEMORY_BUDGET)
}

/// Set the memory budget of the eager decoding, in bytes.
///
/// A budget of `0` refuses every clip: the eager decoding is then turned
/// off and the callers stream their clips.
///
/// The setting wins over the [`MEMORY_BUDGET_ENV`] environment variable.
pub fn set_memory_budget(bytes: usize) {
    BUDGET.store(bytes, Ordering::Relaxed);
    BUDGET_IS_SET.store(true, Ordering::Relaxed);
}

/// Forget the budget that was set explicitly: the environment variable (or
/// the default) is used again.
pub fn reset_memory_budget() {
    BUDGET.store(DEFAULT_MEMORY_BUDGET, Ordering::Relaxed);
    BUDGET_IS_SET.store(false, Ordering::Relaxed);
}

/// The budget of the environment variable, when it holds a valid number.
fn budget_from_env() -> Option<usize> {
    std::env::var(MEMORY_BUDGET_ENV)
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()
}

/// The estimated size of the frames of a clip, in bytes.
///
/// The estimate is the size of one frame times the number of the frames of
/// the clip.  A YUV 4:2:0 frame holds 1.5 bytes per pixel (the luma
/// plane and the two half sized chroma planes), an RGBA frame 4.
///
/// A clip whose frame count is unknown has an estimate of zero: the
/// budget is then checked while the clip is decoded.
pub fn estimate_decoded_size(info: &VideoInfo) -> usize {
    frame_size(info).saturating_mul(info.frame_count as usize)
}

/// The size of one frame of a clip, in bytes.
///
/// The YUV frames are 2.7 times smaller than the RGBA ones: the budget
/// has to know it, otherwise the eager decoding would be refused far too
/// early.
fn frame_size(info: &VideoInfo) -> usize {
    let pixels = (info.width as usize).saturating_mul(info.height as usize);
    if info.is_yuv420() {
        // The luma plane and the two chroma planes, which are half the
        // width and half the height of the frame.
        pixels.saturating_mul(3) / 2
    } else {
        pixels.saturating_mul(4)
    }
}

/// Decode a whole clip into memory.
///
/// The clip is decoded once, frame by frame: the frames are returned in
/// presentation order, together with the metadata of the clip.  The caller
/// can then play them back by index, without decoding anything again.
///
/// The decoding is refused when the clip does not fit into the memory
/// budget (see [`memory_budget`]): the caller gets a
/// [`VideoError::BudgetExceeded`] and can fall back to the streaming
/// playback of the [`VideoPlayer`](crate::pkg_parser::video_parser::player::VideoPlayer).
pub fn decode_all(bytes: &[u8]) -> Result<DecodedClip, VideoError> {
    decode_all_with_budget(bytes, memory_budget())
}

/// Decode a whole clip into memory, with an explicit memory budget.
///
/// The budget is the number of bytes the decoded frames may use.  A clip
/// that needs more is refused with a [`VideoError::BudgetExceeded`].
pub fn decode_all_with_budget(bytes: &[u8], budget: usize) -> Result<DecodedClip, VideoError> {
    let backend = crate::pkg_parser::video_parser::backend_for(bytes)?;
    decode_all_with_backend(backend, budget)
}

/// Decode a whole clip into memory, with a caller-supplied backend.
///
/// The backend is the extension point of the crate: a backend that decodes
/// asynchronously (the ones of the browser) cannot be drained here and
/// reports a [`VideoError::Unsupported`].
pub fn decode_all_with_backend(
    mut backend: Box<dyn FrameDecoder>,
    budget: usize,
) -> Result<DecodedClip, VideoError> {
    let mut info = backend.probe()?;

    // The size of the clip is estimated *before* it is decoded: a clip
    // that does not fit into the budget is refused right away, without
    // decoding a single frame.
    let estimate = estimate_decoded_size(&info);
    if estimate > budget {
        return Err(budget_error(estimate, budget));
    }

    let mut frames = Vec::new();
    let mut decoded_size = 0usize;
    loop {
        match backend.next_frame()? {
            FrameStatus::Frame(frame) => {
                decoded_size = decoded_size.saturating_add(frame.pixel_bytes());
                // The estimate was only an estimate (the frame count of a
                // clip is not always known): the clip is stopped when it
                // turns out to be bigger than the budget.
                if decoded_size > budget {
                    return Err(budget_error(decoded_size, budget));
                }
                frames.push(frame);
            }
            // A backend that decodes asynchronously cannot be drained
            // here: the caller has to use the streaming player.
            FrameStatus::Pending => {
                return Err(VideoError::Unsupported(
                    "the backend decodes asynchronously: the clip cannot be decoded in one go"
                        .to_string(),
                ));
            }
            FrameStatus::End => break,
        }
    }

    debug!(
        "decoded {} frames of a {}x{} clip ({} bytes)",
        frames.len(),
        info.width,
        info.height,
        decoded_size
    );

    // The number of the frames is known exactly now.
    info.frame_count = frames.len() as u32;

    Ok(DecodedClip { frames, info })
}

/// Build the error of a clip that does not fit into the budget.
fn budget_error(size: usize, budget: usize) -> VideoError {
    VideoError::BudgetExceeded(format!(
        "the decoded clip would need {size} bytes, but the budget is {budget} bytes"
    ))
}
