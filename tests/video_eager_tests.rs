//! Integration tests for the eager (bulk) decoding of the videos.
//!
//! The tests cover
//!
//! * the eager decoding of a whole clip: the number of the frames, their
//!   times and their pixels,
//! * the memory budget: a clip that does not fit into it is refused, so
//!   that the caller can stream the clip instead.

use image::codecs::gif::GifEncoder;
use image::{Delay, Frame, RgbaImage};
use pkg_parser::pkg_parser::video_parser::{
    DEFAULT_MEMORY_BUDGET, decode_all_with_budget, memory_budget, reset_memory_budget,
    set_memory_budget,
};

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

/// The whole clip is decoded in one go.
///
/// The clip of the test data is a 32x32 h264 video of four frames.
#[cfg(feature = "ffmpeg")]
#[test]
fn the_whole_clip_is_decoded_at_once() {
    use pkg_parser::pkg_parser::video_parser::VideoPlayer;

    let bytes = include_bytes!("data/sample.mp4");
    let clip =
        decode_all_with_budget(bytes, DEFAULT_MEMORY_BUDGET).expect("the clip must be decoded");

    // The four frames of the clip are decoded.
    assert_eq!(clip.frames.len(), 4);
    assert_eq!(clip.len(), 4);
    assert!(!clip.is_empty());
    assert_eq!(clip.info.width, 32);
    assert_eq!(clip.info.height, 32);
    assert_eq!(clip.info.frame_count, 4);

    // Every frame is at its own time.
    let times = clip.times();
    for (index, time) in times.iter().enumerate() {
        let expected = (index as f64) * 0.1;
        assert!(
            (time - expected).abs() < 1e-3,
            "unexpected time {time} (expected {expected})"
        );
    }

    // The frames are tightly packed RGBA8 frames.
    for frame in &clip.frames {
        assert_eq!(frame.width, 32);
        assert_eq!(frame.height, 32);
        assert_eq!(frame.rgba.len(), 32 * 32 * 4);
    }

    // The pixels are the ones of the test pattern: the corner of the
    // frame is black and the frame holds the yellow block of the
    // pattern.
    let corner = &clip.frames[0].rgba[0..4];
    assert!(
        corner[0] < 16 && corner[1] < 16 && corner[2] < 16,
        "the corner of the frame must be black: {corner:?}"
    );
    let yellow = clip.frames[0]
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[0] > 180 && pixel[1] > 180 && pixel[2] < 80);
    assert!(
        yellow,
        "the frame must hold the yellow block of the test pattern"
    );

    // The test pattern changes from one frame to the next.
    assert_ne!(
        clip.frames[2].rgba, clip.frames[3].rgba,
        "the test pattern changes from one frame to the next"
    );

    // The eagerly decoded frames are the ones the streaming player hands
    // out: the decoding itself did not change.
    let mut player = VideoPlayer::open(bytes).expect("cannot open the sample video");
    let streamed = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(streamed.rgba, clip.frames[0].rgba);
    assert!(
        (streamed.time - clip.frames[0].time).abs() < 1e-9,
        "the times of the frames must be the same"
    );
}

/// A clip that does not fit into the budget is refused.
///
/// The caller then streams the clip instead of decoding it in one go.
#[cfg(feature = "ffmpeg")]
#[test]
fn a_clip_that_does_not_fit_into_the_budget_is_refused() {
    use pkg_parser::pkg_parser::video_parser::{VideoError, VideoPlayer};

    let bytes = include_bytes!("data/sample.mp4");

    // The budget is way too small for the clip (32 * 32 * 4 * 4 bytes).
    let error =
        decode_all_with_budget(bytes, 1024).expect_err("the clip must not be decoded eagerly");
    assert!(
        matches!(&error, VideoError::BudgetExceeded(_)),
        "unexpected error: {error}"
    );

    // The clip can still be streamed: the player does not care about the
    // budget of the eager decoding.
    let mut player = VideoPlayer::open(bytes).expect("cannot open the sample video");
    assert!(player.frame_at(0.0).unwrap().is_some());
}

/// The GIF backend is decoded in one go as well.
#[test]
fn a_gif_is_decoded_in_one_go() {
    let bytes = build_gif();
    let clip =
        decode_all_with_budget(&bytes, DEFAULT_MEMORY_BUDGET).expect("the GIF must be decoded");

    assert_eq!(clip.frames.len(), 3);
    assert_eq!(clip.info.width, 4);
    assert_eq!(clip.info.height, 4);

    for (index, frame) in clip.frames.iter().enumerate() {
        assert_eq!(frame.rgba.len(), 4 * 4 * 4);
        let expected = (index as f64) * 0.1;
        assert!(
            (frame.time - expected).abs() < 1e-6,
            "unexpected time {} (expected {expected})",
            frame.time
        );
    }
}

/// The budget of the eager decoding can be changed by the program.
#[test]
fn the_budget_of_the_eager_decoding_can_be_changed() {
    let default = memory_budget();

    set_memory_budget(1234);
    assert_eq!(memory_budget(), 1234);

    reset_memory_budget();
    assert_eq!(memory_budget(), default);
}
