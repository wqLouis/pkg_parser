//! Integration tests for the browser backends.
//!
//! The tests are only compiled for `wasm32-unknown-unknown`: they need a
//! browser (a media element, a canvas and the WebCodecs `ImageDecoder`).
//! They are checked by `cargo check --target wasm32-unknown-unknown
//! --all-targets`, but they can only be *run* in a browser.
//!
//! The tests use the sample clip of the crate: the media element of the
//! page decodes it, the frames are read back from a canvas.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use pkg_parser::pkg_parser::video_parser::{
    BackendKind, FrameDecoder, FrameStatus, WebVideoBackend, probe_mp4,
};

/// The mp4 header of the sample clip is read without the browser.
#[test]
fn the_sample_clip_is_probed_without_the_browser() {
    let meta = probe_mp4(include_bytes!("data/sample.mp4")).expect("the sample is not an mp4");
    assert_eq!(meta.width, 32);
    assert_eq!(meta.height, 32);
    assert!((meta.duration - 0.4).abs() < 1e-3);
}

/// The media element of the page decodes the video.
///
/// The frames themselves are only available in a browser: the test checks
/// that the backend is wired up and that it knows the size of the clip.
#[test]
fn the_browser_decodes_the_video() {
    let bytes = Arc::from(&include_bytes!("data/sample.mp4")[..]);
    let mut backend = WebVideoBackend::new(bytes, "video/mp4");
    assert_eq!(backend.name(), "web-video");

    // The size of the video is known right away: it comes from the mp4
    // header, the media element does not have to be ready for that.
    let info = backend.probe().expect("cannot probe the video");
    assert_eq!(info.width, 32);
    assert_eq!(info.height, 32);
    assert!(info.duration > 0.0);

    // The browser needs a moment to decode the first frame: until then the
    // backend is pending.
    match backend.next_frame().expect("cannot decode the video") {
        FrameStatus::Pending => {}
        FrameStatus::Frame(frame) => {
            assert_eq!(frame.width, 32);
            assert_eq!(frame.height, 32);
            assert_eq!(frame.rgba().expect("no pixels").len(), 32 * 32 * 4);
        }
        FrameStatus::End => panic!("the video is over already"),
    }
}

/// The browser backends can be selected explicitly.
#[test]
fn the_browser_backends_can_be_selected() {
    assert_eq!(BackendKind::WebVideo.name(), "web-video");
    assert_eq!(BackendKind::WebImage.name(), "web-image");
}
