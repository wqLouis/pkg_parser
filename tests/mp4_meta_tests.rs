//! Integration tests for the MP4 header parser.
//!
//! The parser is pure Rust: it is compiled for every target, so it is
//! tested on the host.  It answers the questions a renderer asks before it
//! can display a video — how big is it and how long is it — without
//! decoding anything.

use pkg_parser::pkg_parser::video_parser::mp4_meta::probe_mp4;

/// The metadata of the sample clip is read out of its header.
///
/// The sample is a 32x32 h264 clip of 0.4 seconds.
#[test]
fn the_sample_clip_is_probed() {
    let meta = probe_mp4(include_bytes!("data/sample.mp4")).expect("the sample is not an mp4");

    assert_eq!(meta.width, 32);
    assert_eq!(meta.height, 32);
    assert!(
        (meta.duration - 0.4).abs() < 1e-3,
        "unexpected duration: {}",
        meta.duration
    );
}

/// The bytes that are not an mp4 are not probed.
#[test]
fn the_garbage_is_not_an_mp4() {
    assert_eq!(probe_mp4(b"this is not a video"), None);
    assert_eq!(probe_mp4(&[]), None);
    assert_eq!(probe_mp4(&[0x00]), None);

    // A file that starts like an mp4 but holds no `moov` box.
    let mut truncated = include_bytes!("data/sample.mp4")[..64].to_vec();
    truncated[4..8].copy_from_slice(b"ftyp");
    assert_eq!(probe_mp4(&truncated), None);
}

/// A file without a video track carries no usable metadata.
#[test]
fn a_file_without_a_video_track_is_not_probed() {
    // A well formed `moov` box that holds no track at all.
    let mut bytes = Vec::new();
    // An empty `moov` box.
    bytes.extend_from_slice(&8u32.to_be_bytes());
    bytes.extend_from_slice(b"moov");
    assert_eq!(probe_mp4(&bytes), None);

    // A `trak` box without a track header.
    let mut trak = Vec::new();
    trak.extend_from_slice(&8u32.to_be_bytes());
    trak.extend_from_slice(b"trak");
    bytes.clear();
    bytes.extend_from_slice(&(8 + trak.len() as u32).to_be_bytes());
    bytes.extend_from_slice(b"moov");
    bytes.extend_from_slice(&trak);
    assert_eq!(probe_mp4(&bytes), None);
}

/// The parser does not need the whole file: the `moov` box is enough.
///
/// The sample clip stores its media data *before* the `moov` box, so the
/// first bytes of the file hold no metadata at all.
#[test]
fn only_the_header_is_needed() {
    let bytes = include_bytes!("data/sample.mp4");

    // The `moov` box starts at the end of the file.
    let meta = probe_mp4(bytes).expect("the sample is not an mp4");
    assert_eq!(meta.width, 32);
    assert_eq!(meta.height, 32);

    // A file that holds nothing but the header of the track carries the
    // same metadata: the parser only reads the boxes it needs and ignores
    // the media data.
    let header_only = &bytes[bytes.len() - 817..];
    assert_eq!(probe_mp4(header_only), Some(meta));

    // The media data alone holds no metadata at all.
    assert_eq!(probe_mp4(&bytes[..1171]), None);
}
