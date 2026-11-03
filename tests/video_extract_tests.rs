//! Integration tests for the extraction of the video frames.
//!
//! The tests build a package in memory, save it with `parse_video` enabled
//! and check what ends up on the disk:
//!
//! * the frames of a GIF (every frame) and of a video (a sample of the clip),
//! * the `<stem>.video.json` sidecar describing the clip,
//! * the fact that a dry run writes nothing at all.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::codecs::gif::GifEncoder;
use image::{Delay, Frame, RgbaImage};
use pkg_parser::pkg_parser::parser::Pkg;
use pkg_parser::pkg_parser::video_parser;

/// A temporary directory that is removed when the test ends.
struct TempDir {
    /// The path of the directory.
    path: PathBuf,
}

impl TempDir {
    /// Create a temporary directory with a unique name.
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is before the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pkg_parser_video_extract_{name}_{}_{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("cannot create the temporary directory");
        Self { path }
    }

    /// The path of the directory.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Build a `.pkg` archive in memory.
///
/// The layout is the one of the Wallpaper Engine packages: the version
/// string, the number of files, the file table (the path, the offset and the
/// size of every file) and finally the data of the files.
fn build_pkg(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buffer = Vec::new();

    // The header: the version string and the number of files.
    let version = b"1.0.0.0";
    buffer.extend_from_slice(&(version.len() as u32).to_le_bytes());
    buffer.extend_from_slice(version);
    buffer.extend_from_slice(&(files.len() as u32).to_le_bytes());

    // The file table: the path, the offset and the size of every file.  The
    // offsets are relative to the end of the table.
    let mut offset = 0u32;
    for (path, data) in files {
        buffer.extend_from_slice(&(path.len() as u32).to_le_bytes());
        buffer.extend_from_slice(path.as_bytes());
        buffer.extend_from_slice(&offset.to_le_bytes());
        buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        offset += data.len() as u32;
    }

    // The data of the files.
    for (_, data) in files {
        buffer.extend_from_slice(data);
    }

    buffer
}

/// Build a package holding a single file.
fn build_single_file_pkg(path: &str, data: Vec<u8>) -> Pkg {
    let buffer = build_pkg(&[(path, data)]);
    Pkg::from_bytes(&buffer).expect("cannot parse the package")
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

/// Read the metadata sidecar of a clip.
fn read_metadata(target: &Path, name: &str) -> serde_json::Value {
    let path = target.join(name);
    assert!(path.is_file(), "the metadata {} is missing", path.display());
    let content = fs::read_to_string(&path).expect("cannot read the metadata");
    serde_json::from_str(&content).expect("the metadata is not valid JSON")
}

/// Every frame of a GIF is extracted and the metadata sidecar is written.
#[test]
fn the_gif_frames_and_the_metadata_are_extracted() {
    let gif = build_gif();
    let pkg = build_single_file_pkg("videos/animation.gif", gif.clone());

    let output = TempDir::new("gif");
    let target = output.path().join("out");
    pkg.save_pkg(&target, false, false, true, false);

    // The raw file is written.
    assert!(target.join("videos/animation.gif").is_file());

    // Every frame of the animation is extracted.
    for index in 0..3 {
        let path = target.join(format!("videos/animation_frame_{index:04}.png"));
        assert!(path.is_file(), "the frame {index} is missing");
    }

    // The frames are the ones the GIF decoder produces.
    let expected = video_parser::save_gif_frames(&gif, "animation").expect("cannot decode the GIF");
    assert_eq!(expected.len(), 3);
    for (data, suffix) in &expected {
        let path = target.join(format!("videos/{suffix}"));
        assert_eq!(
            fs::read(&path).expect("cannot read the frame"),
            *data,
            "the frame {suffix} differs"
        );
    }

    // The metadata sidecar is written next to the frames.
    let metadata = read_metadata(&target, "videos/animation.video.json");
    assert_eq!(metadata["format"], "gif");
    assert_eq!(metadata["width"], 4);
    assert_eq!(metadata["height"], 4);
    assert_eq!(metadata["frame_count"], 3);
    assert_eq!(metadata["extracted_frames"], 3);
    assert_eq!(metadata["has_audio"], false);
    assert_eq!(metadata["codec"], "gif");
    assert!(
        metadata["duration"].as_f64().expect("no duration") > 0.0,
        "the duration is missing"
    );
}

/// The mp4 file is extracted: the frames are decoded when a decoder is
/// available (the `ffmpeg` cargo feature) and the raw file is written in any
/// case.
#[test]
fn the_mp4_frames_and_the_metadata_are_extracted() {
    let mp4 = include_bytes!("data/sample.mp4").to_vec();
    let pkg = build_single_file_pkg("videos/clip.mp4", mp4);

    let output = TempDir::new("mp4");
    let target = output.path().join("out");
    pkg.save_pkg(&target, false, false, true, false);

    // The raw file is always written.
    assert!(target.join("videos/clip.mp4").is_file());

    // The metadata sidecar is written next to the frames.
    let metadata = read_metadata(&target, "videos/clip.video.json");
    assert_eq!(metadata["format"], "mp4");

    if cfg!(feature = "ffmpeg") {
        // The clip is decoded and its metadata is complete.
        assert_eq!(metadata["width"], 32);
        assert_eq!(metadata["height"], 32);
        assert_eq!(metadata["codec"], "h264");
        assert_eq!(metadata["pixel_format"], "yuv420p");
        assert_eq!(metadata["has_audio"], false);
        assert!(metadata["fps"].as_f64().expect("no fps") > 0.0);
        assert!(metadata["duration"].as_f64().expect("no duration") > 0.0);

        // The sampled frames are written; there are at most 24 of them.
        let extracted = metadata["extracted_frames"]
            .as_u64()
            .expect("no frame count");
        let frame_count = metadata["frame_count"].as_u64().expect("no frame count");
        assert!(extracted >= 1);
        assert_eq!(extracted, frame_count.min(24));
        for index in 0..extracted {
            let path = target.join(format!("videos/clip_frame_{index:04}.png"));
            assert!(path.is_file(), "the frame {index} is missing");
        }

        // The frames carry the pixels of the video.
        let frame = image::open(target.join("videos/clip_frame_0000.png"))
            .expect("cannot open the extracted frame");
        assert_eq!(frame.width(), 32);
        assert_eq!(frame.height(), 32);
    } else {
        // Without a decoder only the raw file is written.
        assert_eq!(metadata["extracted_frames"], 0);
        assert!(!target.join("videos/clip_frame_0000.png").exists());
    }
}

/// A dry run writes nothing at all.
#[test]
fn the_dry_run_writes_nothing() {
    let gif = build_gif();
    let mp4 = include_bytes!("data/sample.mp4").to_vec();
    let pkg = Pkg::from_bytes(&build_pkg(&[
        ("videos/animation.gif", gif),
        ("videos/clip.mp4", mp4),
    ]))
    .expect("cannot parse the package");

    let output = TempDir::new("dry_run");
    let target = output.path().join("out");
    pkg.save_pkg(&target, true, false, true, false);

    // Not even the output directory is created.
    assert!(!target.exists(), "the dry run created {target:?}");

    // And nothing was written in the temporary directory either.
    let entries = fs::read_dir(output.path())
        .expect("cannot read the temporary directory")
        .count();
    assert_eq!(entries, 0);
}
