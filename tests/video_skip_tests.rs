//! Integration tests for the skipping of the frames that are never shown.
//!
//! A clip that is recorded at 60 fps and shown at 30 fps holds half of
//! its frames for nothing: they are decoded and thrown away right
//! after.  The ffmpeg decoder can be told to skip the frames that are
//! not reference frames, so that they are not decoded at all.
//!
//! The tests cover
//!
//! * the decoder: the frames that are not shown are not decoded,
//! * the player: the frames it hands out still cover the times it is
//!   asked for, even when some of the frames do not exist.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pkg_parser::pkg_parser::video_parser::{
    FrameDecoder, FramePixels, FrameStatus, VideoError, VideoFrame, VideoInfo, VideoPlayer,
};

/// A clip of 30 frames recorded at 60 fps.
///
/// The clip is encoded with B-frames: half of the frames are not
/// reference frames, so the decoder can skip them.
#[cfg(feature = "ffmpeg")]
const SKIP_CLIP: &[u8] = include_bytes!("data/skip_bframes.mp4");

/// Decode a whole clip and return the times of the frames.
///
/// The frames are decoded one by one, exactly like the eager decoding
/// does it.
#[cfg(feature = "ffmpeg")]
fn decoded_times(backend: &mut dyn FrameDecoder) -> Vec<f64> {
    let mut times = Vec::new();
    loop {
        match backend.next_frame().expect("cannot decode the frame") {
            FrameStatus::Frame(frame) => times.push(frame.time),
            FrameStatus::End => break,
            FrameStatus::Pending => panic!("the ffmpeg decoder is never pending"),
        }
    }
    times
}

/// The frames that are never shown are not decoded at all.
///
/// The clip of the test data holds 30 frames at 60 fps: the frames that
/// are not reference frames are the ones that are never shown when the
/// clip is played at 30 fps.
#[cfg(feature = "ffmpeg")]
#[test]
fn the_frames_that_are_never_shown_are_not_decoded() {
    use pkg_parser::pkg_parser::video_parser::FfmpegBackend;

    // Every frame of the clip is decoded when the skipping is off.
    let mut backend = FfmpegBackend::new(Arc::from(SKIP_CLIP));
    let all = decoded_times(&mut backend);
    assert_eq!(all.len(), 30, "the clip holds thirty frames");
    assert!(
        all.windows(2).all(|window| window[0] < window[1]),
        "the times of the frames must increase: {all:?}"
    );

    // The frames that are not reference frames are not decoded when the
    // skipping is on: the decode count halves.
    let mut backend = FfmpegBackend::new(Arc::from(SKIP_CLIP));
    backend.set_frame_skipping(true);
    let skipped = decoded_times(&mut backend);

    assert!(
        skipped.len() < all.len(),
        "the frames that are never shown must not be decoded: {} of {}",
        skipped.len(),
        all.len()
    );
    assert!(
        skipped.len() * 2 <= all.len() + 2,
        "the decode count must halve: {} of {}",
        skipped.len(),
        all.len()
    );

    // The frames that are decoded keep their own times: the frames that
    // are skipped leave a hole in the stream.
    assert!(
        skipped.windows(2).all(|window| window[0] < window[1]),
        "the times of the frames must increase: {skipped:?}"
    );
    assert!(
        skipped.iter().all(|time| all.contains(time)),
        "the times of the decoded frames must be the times of the clip: {skipped:?}"
    );
    assert!(
        skipped.len() * 2 > all.len() - 4,
        "the frames that are decoded must be the reference ones: {skipped:?}"
    );
}

/// The player hands out the frame that covers the time it is asked for.
///
/// The frames that do not exist (the ones that were skipped) leave holes
/// in the stream: the frame that covers a time in a hole is the frame
/// before it.
#[cfg(feature = "ffmpeg")]
#[test]
fn the_player_hands_out_the_frame_that_covers_the_time() {
    let mut player = VideoPlayer::open(SKIP_CLIP).expect("cannot open the clip");
    let info = player.info().clone();
    assert_eq!(info.fps, 60.0, "the clip is recorded at 60 fps");
    assert_eq!(info.frame_count, 30);

    // The clip is played at 30 fps: half of the frames are never shown.
    player.set_skip_unused_frames(true);

    let mut last = f64::NEG_INFINITY;
    for step in 0..15 {
        let time = step as f64 / 30.0;
        let frame = player
            .frame_at(time)
            .expect("cannot decode")
            .expect("no frame");

        // The frame covers the time: it is the last one before it.
        assert!(
            frame.time <= time + 1e-6,
            "the frame at {time} must not be in the future: {}",
            frame.time
        );
        assert!(
            frame.time > last,
            "the frames must move forward: {} then {}",
            last,
            frame.time
        );
        last = frame.time;
    }

    // The frames that are shown are the ones the decoder handed out: the
    // frames in between do not exist.
    assert!(
        (last - 0.466_666_666_666_666_6).abs() < 1e-6,
        "unexpected time {last}"
    );
}

/// A backend that skips the frames that are not shown.
///
/// It mimics what the ffmpeg decoder does with its `skip_frame` option:
/// the frames that are not reference frames are not decoded at all.
struct SkippingBackend {
    /// The frames of the clip.
    frames: Vec<VideoFrame>,
    /// The index of the next frame.
    next: usize,
    /// Whether the frames that are not shown are skipped.
    skipping: bool,
    /// How many frames were decoded (shared with the test).
    decoded: Arc<AtomicUsize>,
}

impl SkippingBackend {
    /// Build a backend with six frames at 60 fps.
    fn new(decoded: Arc<AtomicUsize>) -> Self {
        let frames = (0..6)
            .map(|index| VideoFrame {
                width: 2,
                height: 2,
                time: index as f64 / 60.0,
                duration: 1.0 / 60.0,
                pixels: FramePixels::Rgba(Arc::from(vec![index as u8; 2 * 2 * 4])),
            })
            .collect();

        Self {
            frames,
            next: 0,
            skipping: false,
            decoded,
        }
    }
}

impl FrameDecoder for SkippingBackend {
    fn name(&self) -> &'static str {
        "skipping"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        Ok(VideoInfo {
            width: 2,
            height: 2,
            duration: 0.1,
            fps: 60.0,
            frame_count: 6,
            codec: "skipping".to_string(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
        // The frames that are not shown are the odd ones: they are not
        // decoded at all.
        while self.skipping && self.next % 2 == 1 {
            self.next += 1;
        }

        match self.frames.get(self.next).cloned() {
            Some(frame) => {
                self.next += 1;
                self.decoded.fetch_add(1, Ordering::Relaxed);
                Ok(FrameStatus::Frame(frame))
            }
            None => Ok(FrameStatus::End),
        }
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        self.next = 0;
        Ok(())
    }

    fn set_frame_skipping(&mut self, enabled: bool) {
        self.skipping = enabled;
    }
}

/// The player asks the decoder to skip the frames that are never shown.
///
/// The frames that do not exist leave holes in the stream: the player
/// hands out the frame that covers the time it is asked for, and it does
/// not think that the stream is over.
#[test]
fn the_player_asks_the_decoder_to_skip_the_unused_frames() {
    let decoded = Arc::new(AtomicUsize::new(0));
    let mut player =
        VideoPlayer::open_with_backend(&[], Box::new(SkippingBackend::new(decoded.clone())))
            .expect("cannot open the backend");

    // The decoder is asked to skip the frames that are never shown.
    player.set_skip_unused_frames(true);

    // The first frame covers the first times: the second frame of the
    // clip does not exist.
    let first = player
        .frame_at(0.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(first.time, 0.0);
    let covered = player
        .frame_at(1.0 / 60.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(
        covered.time, 0.0,
        "the frame that covers a hole is the frame before it"
    );

    // The third frame of the clip exists: it covers its own time.
    let third = player
        .frame_at(2.0 / 60.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(third.time, 2.0 / 60.0);

    // The stream is not over: the frames that are left are still
    // handed out.
    let last = player
        .frame_at(4.0 / 60.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(last.time, 4.0 / 60.0);

    // Half of the frames were decoded: the ones that are never shown
    // were not decoded at all.
    assert_eq!(
        decoded.load(Ordering::Relaxed),
        3,
        "only the frames that are shown must be decoded"
    );
}
