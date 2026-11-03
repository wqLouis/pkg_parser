//! Integration tests for the YUV 4:2:0 support of `pkg_parser`.
//!
//! The tests cover
//!
//! * the extraction of the planes of a decoded frame (their size and the
//!   padding of the rows),
//! * the detection of the colour space of a frame,
//! * the CPU conversion of the planes into RGBA, which has to produce the
//!   same pixels as the `swscale` conversion the parser used to do.

use pkg_parser::pkg_parser::video_parser::yuv::plane_from_rows;
use pkg_parser::pkg_parser::video_parser::{
    FramePixels, VideoFrame, YuvColorspace, YuvMatrix, YuvRange,
};

/// The size of the sample frames.
const WIDTH: u32 = 32;
const HEIGHT: u32 = 32;

/// A plane of a padded frame is copied without its padding.
///
/// The decoder may align the rows of a plane: the padding must not end up
/// in the pixels the renderer uploads.
#[test]
fn a_padded_plane_is_copied_without_its_padding() {
    // Two rows of three bytes, padded to five bytes.
    let data = [1u8, 2, 3, 0xff, 0xff, 4, 5, 6, 0xff, 0xff];

    let plane = plane_from_rows(&data, 5, 3, 2).expect("the plane cannot be copied");
    assert_eq!(plane.as_ref(), &[1u8, 2, 3, 4, 5, 6]);
}

/// A plane that is too short is refused.
#[test]
fn a_truncated_plane_is_refused() {
    let data = [1u8, 2, 3, 4, 5, 6];

    assert!(plane_from_rows(&data, 3, 3, 2).is_some());
    assert!(plane_from_rows(&data, 3, 3, 3).is_none());
    assert!(plane_from_rows(&data, 2, 3, 2).is_none());
}

/// The colour spaces have the coefficients of the standard.
///
/// The values are the ones of the BT.601 and BT.709 matrices with the
/// limited and the full range.
#[test]
fn the_colour_spaces_have_the_standard_coefficients() {
    let bt601 = YuvColorspace::new(YuvMatrix::Bt601, YuvRange::Limited).coefficients();
    assert!((bt601.y_scale - 255.0 / 219.0).abs() < 1e-6);
    assert!((bt601.y_offset - 16.0).abs() < 1e-6);
    assert!((bt601.crv - 1.596027).abs() < 1e-5);
    assert!((bt601.cgu + 0.391762).abs() < 1e-5);
    assert!((bt601.cgv + 0.812968).abs() < 1e-5);
    assert!((bt601.cbu - 2.017232).abs() < 1e-5);

    let bt709 = YuvColorspace::new(YuvMatrix::Bt709, YuvRange::Limited).coefficients();
    assert!((bt709.y_scale - 255.0 / 219.0).abs() < 1e-6);
    assert!((bt709.crv - 1.792741).abs() < 1e-5);
    assert!((bt709.cgu + 0.213248).abs() < 1e-5);
    assert!((bt709.cgv + 0.532909).abs() < 1e-5);
    assert!((bt709.cbu - 2.112402).abs() < 1e-5);

    // The full range has no offsets and no scaling.
    let full = YuvColorspace::new(YuvMatrix::Bt601, YuvRange::Full).coefficients();
    assert!((full.y_scale - 1.0).abs() < 1e-6);
    assert!((full.y_offset - 0.0).abs() < 1e-6);
    assert!((full.crv - 1.402).abs() < 1e-5);
    assert!((full.cgu + 0.344136).abs() < 1e-5);
    assert!((full.cgv + 0.714136).abs() < 1e-5);
    assert!((full.cbu - 1.772).abs() < 1e-5);
}

/// The black of the limited range is black.
#[test]
fn the_black_of_the_colour_spaces_is_black() {
    for (colorspace, black) in [
        (YuvColorspace::Bt601Limited, 16.0),
        (YuvColorspace::Bt601Full, 0.0),
        (YuvColorspace::Bt709Limited, 16.0),
        (YuvColorspace::Bt709Full, 0.0),
    ] {
        let coefficients = colorspace.coefficients();
        let rgb = coefficients.convert(black, 128.0, 128.0);
        assert!(rgb[0].abs() < 0.001, "{:?}: {rgb:?}", colorspace);
        assert!(rgb[1].abs() < 0.001, "{:?}: {rgb:?}", colorspace);
        assert!(rgb[2].abs() < 0.001, "{:?}: {rgb:?}", colorspace);
    }
}

/// The conversion of a frame that is already RGBA gives the pixels back.
#[test]
fn an_rgba_frame_is_handed_out_as_it_is() {
    let frame = VideoFrame {
        width: 2,
        height: 2,
        time: 0.0,
        duration: 0.0,
        pixels: FramePixels::Rgba(std::sync::Arc::from(vec![1u8, 2, 3, 4, 5, 6, 7, 8])),
    };

    let rgba = frame.rgba().expect("the frame cannot be converted");
    assert_eq!(rgba.as_ref(), &[1u8, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(frame.pixel_bytes(), 8);
    assert!(!frame.is_yuv420());
}

/// The tests that need the ffmpeg backend.
#[cfg(feature = "ffmpeg")]
mod ffmpeg {
    use pkg_parser::pkg_parser::video_parser::{FramePixels, VideoPlayer, YuvColorspace};

    use super::{HEIGHT, WIDTH};

    /// The YUV planes of the first frame of the sample video.
    ///
    /// The file holds the luma plane (32×32), the U plane and the V plane
    /// (16×16 each), exactly as the decoder hands them out.
    const SAMPLE_YUV: &[u8] = include_bytes!("data/sample_frame0.yuv");

    /// The RGBA pixels of the first frame of the sample video.
    ///
    /// The file holds the 32×32 RGBA pixels the `swscale` conversion
    /// produced.
    const SAMPLE_RGBA: &[u8] = include_bytes!("data/sample_frame0.rgba");

    /// The decoder hands out the planes of the frame, and the conversion
    /// into RGBA produces the pixels the `swscale` conversion produced.
    #[test]
    fn the_decoded_frame_holds_the_planes_of_the_sample() {
        let bytes = include_bytes!("data/sample.mp4");
        let mut player = VideoPlayer::open(bytes).expect("cannot open the sample video");

        let frame = player.frame_at(0.0).unwrap().expect("no frame");
        assert_eq!(frame.width, WIDTH);
        assert_eq!(frame.height, HEIGHT);
        assert!(frame.is_yuv420(), "the frame must hold YUV planes");

        let FramePixels::Yuv420 {
            y,
            u,
            v,
            colorspace,
        } = &frame.pixels
        else {
            panic!("the frame must hold YUV planes");
        };

        // The planes have the size of the frame and of the half sized chroma.
        assert_eq!(y.len(), (WIDTH * HEIGHT) as usize);
        assert_eq!(u.len(), (WIDTH / 2 * HEIGHT / 2) as usize);
        assert_eq!(v.len(), (WIDTH / 2 * HEIGHT / 2) as usize);

        // The sample does not tell anything about its colour space: the
        // default of the parser (BT.601, limited range) is used, which is
        // what the `swscale` conversion assumes as well.
        assert_eq!(*colorspace, YuvColorspace::Bt601Limited);

        // The planes are the ones of the decoded frame.
        let luma = (WIDTH * HEIGHT) as usize;
        let chroma = luma / 4;
        assert_eq!(y.as_ref(), &SAMPLE_YUV[..luma]);
        assert_eq!(u.as_ref(), &SAMPLE_YUV[luma..luma + chroma]);
        assert_eq!(v.as_ref(), &SAMPLE_YUV[luma + chroma..]);
    }

    /// The CPU conversion produces the pixels of the `swscale` conversion.
    ///
    /// The two do not agree bit for bit: the `swscale` conversion uses
    /// fixed point coefficients, the CPU conversion floating point ones.
    /// They agree within three units, which is invisible.
    #[test]
    fn the_conversion_produces_the_swscale_pixels() {
        let bytes = include_bytes!("data/sample.mp4");
        let mut player = VideoPlayer::open(bytes).expect("cannot open the sample video");

        let frame = player.frame_at(0.0).unwrap().expect("no frame");
        let rgba = frame.rgba().expect("the frame cannot be converted");

        assert_eq!(rgba.len(), SAMPLE_RGBA.len());
        for (index, (converted, expected)) in rgba.iter().zip(SAMPLE_RGBA.iter()).enumerate() {
            let difference = (i32::from(*converted) - i32::from(*expected)).abs();
            assert!(
                difference <= 3,
                "the pixel {index} differs too much: {converted} instead of {expected}"
            );
        }
    }
}
