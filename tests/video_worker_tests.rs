//! Integration tests for the worker thread that decodes the frames.
//!
//! The player decodes the frames on a worker thread, so that the
//! decoding does not block the thread that plays the clip back.  The
//! frames the player hands out must be exactly the ones it handed out
//! before, in the same order, and the "pending" status of an
//! asynchronous decoder must still work.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pkg_parser::pkg_parser::video_parser::{
    FrameDecoder, FramePixels, FrameStatus, VideoError, VideoFrame, VideoInfo, VideoPlayer,
};

/// A backend that hands out three synthetic frames at 0.0 / 0.5 / 1.0.
///
/// Every frame is filled with a single byte, so the tests can tell them
/// apart without decoding anything.
struct MockBackend {
    /// The frames the backend hands out.
    frames: Vec<VideoFrame>,
    /// Index of the next frame.
    next: usize,
    /// How many frames were decoded so far (shared with the test).
    decoded: Arc<AtomicUsize>,
    /// How many times the backend was dropped (shared with the test).
    dropped: Arc<AtomicUsize>,
}

impl MockBackend {
    /// Build a backend with three frames.
    fn new(decoded: Arc<AtomicUsize>, dropped: Arc<AtomicUsize>) -> Self {
        let frames = [0u8, 1, 2]
            .into_iter()
            .enumerate()
            .map(|(index, value)| VideoFrame {
                width: 2,
                height: 2,
                time: index as f64 * 0.5,
                duration: 0.5,
                pixels: FramePixels::Rgba(Arc::from(vec![value; 2 * 2 * 4])),
            })
            .collect();

        Self {
            frames,
            next: 0,
            decoded,
            dropped,
        }
    }
}

impl Drop for MockBackend {
    fn drop(&mut self) {
        // The backend is dropped when the worker thread stops.
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

impl FrameDecoder for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        Ok(VideoInfo {
            width: 2,
            height: 2,
            duration: 1.5,
            fps: 2.0,
            frame_count: 3,
            codec: "mock".to_string(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
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
}

/// A backend that reports "pending" before every frame.
///
/// The backend mimics an asynchronous decoder (the browser's WebCodecs,
/// the HTML media element): it cannot hand out a frame on demand, so it
/// answers [`FrameStatus::Pending`] a couple of times before the frame
/// is ready.
struct PendingBackend {
    /// The frames the backend hands out.
    frames: Vec<VideoFrame>,
    /// Index of the next frame.
    next: usize,
    /// How many times the backend reports "pending" before every frame.
    rounds: usize,
    /// How many times the backend still has to report "pending".
    pending: usize,
}

impl PendingBackend {
    /// Build a backend that reports "pending" `rounds` times before every frame.
    fn new(rounds: usize) -> Self {
        let frames = [0u8, 1, 2]
            .into_iter()
            .enumerate()
            .map(|(index, value)| VideoFrame {
                width: 2,
                height: 2,
                time: index as f64 * 0.5,
                duration: 0.5,
                pixels: FramePixels::Rgba(Arc::from(vec![value; 2 * 2 * 4])),
            })
            .collect();

        Self {
            frames,
            next: 0,
            rounds,
            pending: rounds,
        }
    }
}

impl FrameDecoder for PendingBackend {
    fn name(&self) -> &'static str {
        "pending"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        Ok(VideoInfo {
            width: 2,
            height: 2,
            duration: 1.5,
            fps: 2.0,
            frame_count: 3,
            codec: "pending".to_string(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
        // The decoder is still working on the frame: it is not ready yet.
        if self.pending > 0 {
            self.pending -= 1;
            return Ok(FrameStatus::Pending);
        }

        match self.frames.get(self.next).cloned() {
            Some(frame) => {
                self.next += 1;
                // The next frame has to be waited for as well.
                self.pending = self.rounds;
                Ok(FrameStatus::Frame(frame))
            }
            None => Ok(FrameStatus::End),
        }
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        self.next = 0;
        self.pending = self.rounds;
        Ok(())
    }
}

/// The worker hands out the same frames as the decoder itself.
///
/// The frames must come out in the same order, with the same times and
/// the same pixels.
#[test]
fn the_worker_hands_out_the_same_frames() {
    let decoded = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    let mut inline = VideoPlayer::open_with_backend(
        &[],
        Box::new(MockBackend::new(decoded.clone(), dropped.clone())),
    )
    .expect("cannot open the backend");

    let mut worker = VideoPlayer::open_with_worker_backend(
        &[],
        Box::new(MockBackend::new(decoded.clone(), dropped.clone())),
    )
    .expect("cannot open the backend on a worker");

    for step in 0..6 {
        let time = step as f64 * 0.25;
        let expected = inline.frame_at(time).expect("cannot decode");
        let found = worker.frame_at(time).expect("cannot decode");

        match (expected, found) {
            (None, None) => {}
            (Some(expected), Some(found)) => {
                assert_eq!(expected.time, found.time, "the times must be the same");
                assert_eq!(
                    expected.rgba().expect("no pixels"),
                    found.rgba().expect("no pixels"),
                    "the pixels must be the same"
                );
            }
            _ => panic!("the worker did not hand out the same frames"),
        }
    }

    // A backwards jump rewinds the stream: the worker restarts it too.
    let first = worker
        .frame_at(0.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(first.time, 0.0);
    let second = worker
        .frame_at(0.6)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(second.time, 0.5);
}

/// The worker keeps the frame that is on screen while the decoder is
/// busy.
///
/// A decoder that is not ready yet answers "pending": the player then
/// hands out the frame that is on screen, exactly as it does when the
/// decoder is called directly.
#[test]
fn the_worker_keeps_the_frame_that_is_on_screen() {
    let mut player = VideoPlayer::open_with_worker_backend(&[], Box::new(PendingBackend::new(2)))
        .expect("cannot open the backend on a worker");

    // The decoder is not ready yet and no frame was decoded: there is
    // nothing to show.
    assert!(player.frame_at(0.0).expect("cannot decode").is_none());
    assert!(player.frame_at(0.0).expect("cannot decode").is_none());

    // The frame is handed out as soon as it is ready.
    let first = player
        .frame_at(0.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(first.rgba().expect("no pixels")[0], 0);

    // While the decoder is pending, the frame that is on screen stays
    // there.
    let pending = player
        .frame_at(0.6)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(
        pending.rgba().expect("no pixels")[0],
        0,
        "the cached frame must stay on screen"
    );

    // The next frame is served as soon as it is ready.
    let mut second = None;
    for _ in 0..4 {
        second = player.frame_at(0.6).expect("cannot decode");
        if let Some(frame) = &second
            && frame.rgba().expect("no pixels")[0] == 1
        {
            break;
        }
    }
    assert_eq!(
        second.expect("no frame").rgba().expect("no pixels")[0],
        1,
        "the next frame must be served as soon as it is ready"
    );
}

/// The worker decodes the frames ahead of the playback.
///
/// The rate of the playback tells the worker how far ahead it has to
/// decode: the frames are then ready when the player asks for them.
#[test]
fn the_worker_decodes_the_frames_ahead() {
    let decoded = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    let mut player = VideoPlayer::open_with_worker_backend(
        &[],
        Box::new(MockBackend::new(decoded.clone(), dropped.clone())),
    )
    .expect("cannot open the backend on a worker");

    // The clip is played at 30 fps: the worker decodes the frames that
    // come next while the player is busy with something else.
    player.set_target_frame_rate(30.0);

    let first = player
        .frame_at(0.0)
        .expect("cannot decode")
        .expect("no frame");
    assert_eq!(first.time, 0.0);

    // Give the worker the time to decode the frames that come next.
    std::thread::sleep(std::time::Duration::from_millis(100));

    assert!(
        decoded.load(Ordering::Relaxed) > 1,
        "the worker must decode the frames ahead of the playback: {}",
        decoded.load(Ordering::Relaxed)
    );
}

/// The worker is shut down when the player is dropped.
///
/// The decoder lives on the worker thread: it is dropped when the thread
/// stops, which happens when the player is dropped.
#[test]
fn the_worker_is_shut_down_when_the_player_is_dropped() {
    let decoded = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    let mut player = VideoPlayer::open_with_worker_backend(
        &[],
        Box::new(MockBackend::new(decoded.clone(), dropped.clone())),
    )
    .expect("cannot open the backend on a worker");

    let _ = player.frame_at(0.0).expect("cannot decode");
    assert_eq!(
        dropped.load(Ordering::Relaxed),
        0,
        "the worker is still running"
    );

    drop(player);

    assert_eq!(
        dropped.load(Ordering::Relaxed),
        1,
        "the worker must be shut down when the player is dropped"
    );
}
