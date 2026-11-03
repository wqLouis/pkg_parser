//! Integration tests for the still image support of `pkg_parser`.
//!
//! The tests cover
//!
//! * the pure Rust still image backend, driven through the player,
//! * the content sniffing helper that picks the backend for a payload,
//! * the backwards compatible name of the decoder trait.

use std::io::Cursor;
use std::sync::Arc;

use image::codecs::gif::GifEncoder;
use image::{Delay, Frame, RgbaImage};
use pkg_parser::pkg_parser::video_parser::{
    BackendKind, FrameDecoder, ImageBackend, ImageFormat, VideoBackend, VideoError, VideoFrame,
    VideoInfo, VideoPlayer, backend_for, detect_image_format,
};

/// Build a 4x4 png in memory.
fn build_png() -> Vec<u8> {
    let mut picture = RgbaImage::new(4, 4);
    picture.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));

    let mut buffer = Vec::new();
    picture
        .write_to(&mut Cursor::new(&mut buffer), image::ImageFormat::Png)
        .expect("cannot encode the png");
    buffer
}

/// Build a 2 frame GIF (4x4 pixels) in memory.
fn build_gif() -> Vec<u8> {
    let mut buffer = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut buffer);
        for index in 0..2u8 {
            let mut picture = RgbaImage::new(4, 4);
            picture.put_pixel(0, 0, image::Rgba([index * 80, 0, 0, 255]));
            let frame = Frame::from_parts(picture, 0, 0, Delay::from_numer_denom_ms(100, 1));
            encoder
                .encode_frame(frame)
                .expect("cannot encode the frame");
        }
    }
    buffer
}

/// A still image is a stream with exactly one frame.
#[test]
fn a_still_image_is_a_stream_with_one_frame() {
    let bytes = build_png();
    let mut player = VideoPlayer::open_with_backend_kind(&bytes, BackendKind::Image)
        .expect("cannot open the image");
    assert_eq!(player.backend_name(), "image");

    let info = player.info().clone();
    assert_eq!(info.width, 4);
    assert_eq!(info.height, 4);
    assert_eq!(info.frame_count, 1);
    assert_eq!(info.duration, 0.0);
    assert_eq!(info.codec, "png");
    assert!(!info.has_audio);

    let frame = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(frame.width, 4);
    assert_eq!(frame.height, 4);
    assert_eq!(frame.rgba.len(), 4 * 4 * 4);

    // A still image never ends: the same frame is served for any time.
    let later = player.frame_at(5.0).unwrap().expect("no frame");
    assert_eq!(later.rgba, frame.rgba);

    // After a reset the frame is available again.
    player.reset().expect("cannot reset the player");
    let replayed = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(replayed.rgba, frame.rgba);
}

/// The image backend can also be used on its own.
#[test]
fn the_image_backend_decodes_a_single_frame() {
    let bytes = build_png();
    let mut backend = ImageBackend::new(Arc::from(bytes.as_slice()));
    assert_eq!(backend.name(), "image");
    assert_eq!(backend.format(), Some(ImageFormat::Png));

    let info = backend.probe().expect("cannot probe the image");
    assert_eq!(info.width, 4);
    assert_eq!(info.height, 4);
    assert_eq!(info.frame_count, 1);

    let frame = backend.next_frame().unwrap().expect("no frame");
    assert_eq!(frame.width, 4);
    assert_eq!(frame.height, 4);
    assert_eq!(frame.rgba.len(), 4 * 4 * 4);

    // The single frame was handed out: the stream is over.
    assert!(backend.next_frame().unwrap().is_none());

    // A rewind makes the frame available again.
    backend.rewind().expect("cannot rewind the backend");
    let replayed = backend.next_frame().unwrap().expect("no frame");
    assert_eq!(replayed.rgba, frame.rgba);
}

/// The frame can be delivered at another resolution.
#[test]
fn the_image_backend_can_be_rescaled() {
    let bytes = build_png();
    let mut backend = ImageBackend::new(Arc::from(bytes.as_slice()));
    backend.set_output_size(8, 8);

    let frame = backend.next_frame().unwrap().expect("no frame");
    assert_eq!(frame.width, 8);
    assert_eq!(frame.height, 8);
    assert_eq!(frame.rgba.len(), 8 * 8 * 4);
}

/// The image formats are detected from the magic bytes.
#[test]
fn the_image_formats_are_detected() {
    assert_eq!(detect_image_format(&build_png()), Some(ImageFormat::Png));
    assert_eq!(
        detect_image_format(b"\xFF\xD8\xFF\xE0"),
        Some(ImageFormat::Jpeg)
    );
    assert_eq!(
        detect_image_format(b"RIFF\x00\x00\x00\x00WEBP"),
        Some(ImageFormat::WebP)
    );
    assert_eq!(detect_image_format(b"BM\x00\x00"), Some(ImageFormat::Bmp));

    // Neither a video container nor an image.
    assert_eq!(detect_image_format(b"GIF89a"), None);
    assert_eq!(detect_image_format(b"this is not an image"), None);
    assert_eq!(detect_image_format(&[]), None);
}

/// The backend of a payload is picked from its format.
#[test]
fn backend_for_picks_the_right_backend() {
    let gif = backend_for(&build_gif()).expect("no backend for the GIF");
    assert_eq!(gif.name(), "gif");

    let png = backend_for(&build_png()).expect("no backend for the png");
    assert_eq!(png.name(), "image");

    // Garbage has no backend at all.
    let error = match backend_for(b"this is not a video") {
        Ok(_) => panic!("the garbage should not be supported"),
        Err(error) => error,
    };
    assert!(matches!(error, VideoError::Unsupported(_)));
}

/// The mp4 containers are decoded by the ffmpeg backend.
#[cfg(feature = "ffmpeg")]
#[test]
fn backend_for_uses_ffmpeg_for_the_videos() {
    let backend = backend_for(include_bytes!("data/sample.mp4")).expect("no backend");
    assert_eq!(backend.name(), "ffmpeg");
}

/// Without the `ffmpeg` feature there is no backend for the mp4 files.
#[cfg(not(feature = "ffmpeg"))]
#[test]
fn the_videos_need_the_ffmpeg_feature() {
    let error = match backend_for(include_bytes!("data/sample.mp4")) {
        Ok(_) => panic!("the mp4 file should not be supported"),
        Err(error) => error,
    };
    assert!(matches!(error, VideoError::Unsupported(_)));
}

/// A backend written against the old name of the trait.
///
/// [`VideoBackend`] is only an alias of [`FrameDecoder`]: the two names
/// describe the very same trait, so a backend written against either of them
/// can be driven by the player.
struct AliasBackend {
    /// Whether the single frame was already handed out.
    decoded: bool,
}

impl VideoBackend for AliasBackend {
    fn name(&self) -> &'static str {
        "alias"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        Ok(VideoInfo {
            width: 1,
            height: 1,
            duration: 0.0,
            fps: 0.0,
            frame_count: 1,
            codec: "alias".to_string(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, VideoError> {
        if self.decoded {
            return Ok(None);
        }
        self.decoded = true;
        Ok(Some(VideoFrame {
            width: 1,
            height: 1,
            time: 0.0,
            duration: 0.0,
            rgba: Arc::from(vec![0u8; 4]),
        }))
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        self.decoded = false;
        Ok(())
    }
}

/// The old name of the trait still resolves, and the player still works with
/// a backend written against it.
#[test]
fn the_old_trait_name_still_works() {
    let mut player = VideoPlayer::open_with_backend(&[], Box::new(AliasBackend { decoded: false }))
        .expect("cannot open the backend");
    assert_eq!(player.backend_name(), "alias");

    let frame = player.frame_at(0.0).unwrap().expect("no frame");
    assert_eq!(frame.rgba.len(), 4);
}
