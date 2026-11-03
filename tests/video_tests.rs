//! Integration tests for the video support of `pkg_parser`.
//!
//! The tests cover
//!
//! * the container detection (magic bytes),
//! * the backend abstraction: a mock backend proves that the core of the
//!   crate does not depend on ffmpeg (the test runs with and without the
//!   `ffmpeg` cargo feature),
//! * the pure Rust GIF backend,
//! * the ffmpeg based decoder (only with the `ffmpeg` cargo feature).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use image::codecs::gif::GifEncoder;
use image::{Delay, Frame, RgbaImage};
use pkg_parser::pkg_parser::video_parser::{
    BackendKind, FrameDecoder, FramePixels, FrameStatus, Video, VideoError, VideoFormat,
    VideoFrame, VideoInfo, VideoPlayer, detect_format,
};

/// A backend producing three synthetic 2x2 frames at t = 0.0 / 0.5 / 1.0.
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
}

impl MockBackend {
    /// Build a backend with three frames.
    fn new(decoded: Arc<AtomicUsize>) -> Self {
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
        }
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

/// A backend that reports "not ready yet" before every frame.
///
/// The backend mimics an asynchronous decoder (the browser's WebCodecs, the
/// HTML media element): it cannot hand out a frame on demand, so it answers
/// [`FrameStatus::Pending`] a couple of times before the frame is ready.
struct PendingBackend {
    /// The frames the backend hands out.
    frames: Vec<VideoFrame>,
    /// Index of the next frame.
    next: usize,
    /// How many times the backend reports "pending" before every frame.
    rounds: usize,
    /// How many times the backend still has to report "pending".
    pending: usize,
    /// How many times the backend reported "pending" (shared with the test).
    pendings: Arc<AtomicUsize>,
}

impl PendingBackend {
    /// Build a backend that reports "pending" `rounds` times before every frame.
    ///
    /// The counter of the "pending" answers is returned as well, so that the
    /// test can check that the backend was really asked.
    fn new(rounds: usize) -> (Self, Arc<AtomicUsize>) {
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

        let pendings = Arc::new(AtomicUsize::new(0));
        let backend = Self {
            frames,
            next: 0,
            rounds,
            pending: rounds,
            pendings: pendings.clone(),
        };

        (backend, pendings)
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
            self.pendings.fetch_add(1, Ordering::Relaxed);
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

/// Build a 3 frame GIF (4x4 pixels, 100 ms per frame) in memory.
fn build_gif() -> Vec<u8> {
    let mut buffer = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut buffer);
        for index in 0..3u8 {
            let mut image = RgbaImage::new(4, 4);
            image.put_pixel(0, 0, image::Rgba([index * 80, 0, 0, 255]));
            let frame = Frame::from_parts(image, 0, 0, Delay::from_numer_denom_ms(100, 1));
            encoder
                .encode_frame(frame)
                .expect("cannot encode the frame");
        }
    }
    buffer
}

/// The container format is detected from the magic bytes.
#[test]
fn detect_format_recognises_the_containers() {
    // MP4: an `ftyp` box at offset 4.
    assert_eq!(
        detect_format(&[0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p']),
        Some(VideoFormat::Mp4)
    );
    // WebM: an EBML header.
    assert_eq!(
        detect_format(&[0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00]),
        Some(VideoFormat::WebM)
    );
    // GIF: the `GIF87a` and `GIF89a` signatures.
    assert_eq!(detect_format(b"GIF89a"), Some(VideoFormat::Gif));
    assert_eq!(detect_format(b"GIF87a"), Some(VideoFormat::Gif));
    // Garbage is not a video.
    assert_eq!(detect_format(b"this is not a video"), None);
    assert_eq!(detect_format(&[]), None);
}

/// The player only talks to the [`FrameDecoder`] trait, so any backend works.
///
/// This test runs with and without the `ffmpeg` cargo feature.
#[test]
fn the_player_works_with_any_backend() {
    let decoded = Arc::new(AtomicUsize::new(0));
    let mut player =
        VideoPlayer::open_with_backend(&[], Box::new(MockBackend::new(decoded.clone())))
            .expect("cannot open the mock backend");
    assert_eq!(player.backend_name(), "mock");
    assert_eq!(player.info().frame_count, 3);

    // The frame covering a time is the last one before it.
    let first = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(first.rgba().expect("no pixels")[0], 0);

    // The same frame is served from the cache: nothing is decoded again.
    let decoded_frames = decoded.load(Ordering::Relaxed);
    let cached = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(cached.rgba().expect("no pixels")[0], 0);
    assert_eq!(
        decoded.load(Ordering::Relaxed),
        decoded_frames,
        "the frame must come from the cache"
    );

    let second = player.frame_at(0.6).unwrap().expect("no frame");
    assert_eq!(second.rgba().expect("no pixels")[0], 1);

    let third = player.frame_at(1.2).unwrap().expect("no frame");
    assert_eq!(third.rgba().expect("no pixels")[0], 2);

    // A backwards jump rewinds the backend and starts from the first frame.
    let again = player.frame_at(0.1).unwrap().expect("no frame");
    assert_eq!(again.rgba().expect("no pixels")[0], 0);
}

/// An asynchronous decoder that is not ready yet does not stop the playback.
///
/// The backend reports "pending" a couple of times before every frame: the
/// player has to keep the frame that is on screen and hand out the next one
/// as soon as it is ready.
#[test]
fn a_pending_backend_keeps_the_frame_that_is_on_screen() {
    let (backend, pendings) = PendingBackend::new(2);
    let mut player =
        VideoPlayer::open_with_backend(&[], Box::new(backend)).expect("cannot open the backend");

    // The decoder is not ready yet and no frame was decoded: there is
    // nothing to show.
    assert!(player.frame_at(0.0).unwrap().is_none());
    assert!(player.frame_at(0.0).unwrap().is_none());

    // The third call gets the first frame; the decoder is pending again for
    // the next one, so the frame that is on screen is served.
    let first = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(first.rgba().expect("no pixels")[0], 0);

    // While the decoder is pending, the frame that is on screen stays there.
    let pending = player.frame_at(0.6).unwrap().expect("no frame");
    assert_eq!(
        pending.rgba().expect("no pixels")[0],
        0,
        "the cached frame must stay on screen"
    );
    assert!(
        pendings.load(Ordering::Relaxed) > 0,
        "the backend must have been asked for a frame"
    );

    // The next frame is served as soon as it is ready.
    let second = player.frame_at(0.6).unwrap().expect("no frame");
    assert_eq!(second.rgba().expect("no pixels")[0], 1);

    // The stream was not considered finished while the decoder was pending:
    // the last frame is decoded and served.
    let third = player.frame_at(1.2).unwrap().expect("no frame");
    assert_eq!(
        third.rgba().expect("no pixels")[0],
        1,
        "the decoder is still pending"
    );
    let last = player.frame_at(1.2).unwrap().expect("no frame");
    assert_eq!(last.rgba().expect("no pixels")[0], 2);
}

/// The GIF backend decodes every frame of the animation.
#[test]
fn the_gif_backend_decodes_every_frame() {
    let bytes = build_gif();
    let mut player = VideoPlayer::open(&bytes).expect("cannot open the GIF");
    assert_eq!(player.backend_name(), "gif");

    let info = player.info().clone();
    assert_eq!(info.width, 4);
    assert_eq!(info.height, 4);
    assert_eq!(info.frame_count, 3);
    assert!(info.duration > 0.0);
    assert!(info.fps > 0.0);
    assert!(!info.has_audio);

    let mut times = Vec::new();
    for index in 0..3 {
        let frame = player
            .frame_at(index as f64 * 0.1)
            .unwrap()
            .expect("no frame");
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 4);
        assert_eq!(frame.rgba().expect("no pixels").len(), 4 * 4 * 4);
        times.push(frame.time);
    }

    assert!(
        times.windows(2).all(|window| window[0] < window[1]),
        "the frame times must increase: {times:?}"
    );
}

/// The GIF backend can also be selected explicitly.
#[test]
fn the_gif_backend_can_be_selected() {
    let bytes = build_gif();
    let mut player =
        VideoPlayer::open_with_backend_kind(&bytes, BackendKind::Gif).expect("cannot open the GIF");
    assert_eq!(player.backend_name(), "gif");
    assert!(player.frame_at(0.0).unwrap().is_some());
}

/// The legacy API of the module still behaves the same way.
#[test]
fn the_legacy_video_api_still_works() {
    let bytes = build_gif();

    let video = Video::new(&bytes).expect("the GIF was not recognised");
    assert!(video.is_gif());
    assert!(!video.is_video());
    assert_eq!(video.dimensions, Some((4, 4)));
    assert_eq!(video.frame_count, Some(3));
    assert_eq!(video.extract_frame(0).expect("no frame").len(), 4 * 4 * 4);
    assert_eq!(video.extract_all_frames().expect("no frames").len(), 3);

    // A video container is recognised, but it is not decoded yet.
    let video = Video::new(include_bytes!("data/sample.mp4")).expect("not a video");
    assert!(video.is_video());
    assert_eq!(video.dimensions, None);
    assert_eq!(video.frame_count, None);
    assert!(video.extract_frame(0).is_none());
}

/// Without the `ffmpeg` feature there is no decoder for the mp4 files.
#[cfg(not(feature = "ffmpeg"))]
#[test]
fn the_mp4_files_need_the_ffmpeg_feature() {
    let error = match VideoPlayer::open(include_bytes!("data/sample.mp4")) {
        Ok(_) => panic!("the mp4 file should not be supported"),
        Err(error) => error,
    };
    assert!(matches!(error, VideoError::Unsupported(_)));
}

/// The ffmpeg backend decodes the sample video.
#[cfg(feature = "ffmpeg")]
#[test]
fn the_ffmpeg_backend_decodes_the_sample() {
    let bytes = include_bytes!("data/sample.mp4");
    let mut player = VideoPlayer::open(bytes).expect("cannot open the sample video");
    assert_eq!(player.backend_name(), "ffmpeg");

    let info = player.info().clone();
    assert_eq!(info.width, 32);
    assert_eq!(info.height, 32);
    assert!(info.fps > 0.0);
    assert!(info.duration > 0.0);
    assert!(info.frame_count >= 1);
    assert_eq!(info.codec, "h264");
    assert_eq!(info.pixel_format, "yuv420p");

    let first = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(first.width, 32);
    assert_eq!(first.height, 32);
    assert_eq!(first.rgba().expect("no pixels").len(), 32 * 32 * 4);
    assert!(
        (first.time - 0.0).abs() < 1e-3,
        "unexpected time {}",
        first.time
    );

    let second = player.frame_at(0.1).unwrap().expect("no frame");
    assert_eq!(second.rgba().expect("no pixels").len(), 32 * 32 * 4);
    assert!(
        (second.time - 0.1).abs() < 1e-3,
        "unexpected time {}",
        second.time
    );

    let third = player.frame_at(0.2).unwrap().expect("no frame");
    assert_eq!(third.rgba().expect("no pixels").len(), 32 * 32 * 4);
    assert!(
        (third.time - 0.2).abs() < 1e-3,
        "unexpected time {}",
        third.time
    );

    // The test pattern changes from one frame to the next (the first two
    // frames of the `testsrc` pattern happen to be identical).
    let fourth = player.frame_at(0.3).unwrap().expect("no frame");
    assert_eq!(fourth.rgba().expect("no pixels").len(), 32 * 32 * 4);
    assert_ne!(
        third.rgba().expect("no pixels"),
        fourth.rgba().expect("no pixels"),
        "the test pattern changes from one frame to the next"
    );

    // The video can be replayed: after a reset the first frame comes back.
    player.reset().expect("cannot reset the player");
    let replayed = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(
        replayed.rgba().expect("no pixels"),
        first.rgba().expect("no pixels")
    );

    // A time past the end of the stream still yields the last frame.
    player.reset().expect("cannot reset the player");
    let past_the_end = player
        .frame_at(info.duration + 0.05)
        .unwrap()
        .expect("no frame");
    assert_eq!(past_the_end.width, 32);
    assert_eq!(past_the_end.rgba().expect("no pixels").len(), 32 * 32 * 4);
}
