//! Extraction of the real frames of a video and of the metadata sidecar.
//!
//! When a package is saved, the raw `.mp4`/`.webm`/`.gif` files are written
//! next to the other assets.  This module goes one step further: it decodes
//! the clip with the backends of
//! [`video_parser`](crate::pkg_parser::video_parser) and writes the frames as
//! PNG files, together with a small JSON sidecar describing the clip.
//!
//! * A GIF is cheap to decode, so *all* of its frames are written.
//! * A video clip can be much longer, so at most [`MAX_EXTRACTED_FRAMES`]
//!   frames are sampled evenly over the duration of the clip.
//!
//! Nothing in this module panics: a clip that cannot be decoded (no decoder
//! for the container, a broken file, …) only produces a warning and the raw
//! file.

use std::fs::{self, File, create_dir_all};
use std::io::{BufWriter, Write};
use std::path::Path;

use image::{ImageFormat, RgbaImage};
use log::{debug, warn};
use serde::Serialize;

use crate::pkg_parser::video_parser::{self, VideoFormat, VideoFrame, VideoInfo, VideoPlayer};

/// Maximum number of frames extracted from a video clip.
///
/// The frames are sampled evenly over the duration of the clip, so that a
/// long video does not produce thousands of PNG files.
pub const MAX_EXTRACTED_FRAMES: u32 = 24;

/// The metadata of a clip, written next to the extracted frames.
///
/// The structure is serialized as `<stem>.video.json`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct VideoMetadata {
    /// Name of the container: `"mp4"`, `"webm"` or `"gif"`.
    pub format: String,
    /// Width of the clip in pixels.
    pub width: u32,
    /// Height of the clip in pixels.
    pub height: u32,
    /// Length of the clip in seconds.
    pub duration: f64,
    /// Frames per second.
    pub fps: f64,
    /// Number of frames the clip advertises.
    pub frame_count: u32,
    /// Name of the codec (e.g. `"h264"`).
    pub codec: String,
    /// Name of the pixel format (e.g. `"yuv420p"`).
    pub pixel_format: String,
    /// Whether the container carries an audio stream.
    pub has_audio: bool,
    /// Number of frames that were extracted.
    pub extracted_frames: u32,
}

impl VideoMetadata {
    /// Build the metadata out of the information probed by the decoder.
    fn from_info(format: &str, info: &VideoInfo) -> Self {
        Self {
            format: format.to_string(),
            width: info.width,
            height: info.height,
            duration: info.duration,
            fps: info.fps,
            frame_count: info.frame_count,
            codec: info.codec.clone(),
            pixel_format: info.pixel_format.clone(),
            has_audio: info.has_audio,
            extracted_frames: 0,
        }
    }
}

/// The result of an extraction.
#[derive(Debug, Clone)]
pub struct Extraction {
    /// The metadata of the clip.
    pub metadata: VideoMetadata,
    /// The number of frames that were extracted.
    pub frames: u32,
}

/// Extract the frames of a video or of a GIF and write the metadata sidecar.
///
/// The frames are written next to `output_path` (the raw file) and are named
/// `<stem>_frame_0000.png`, `<stem>_frame_0001.png`, …; the metadata is
/// written to `<stem>.video.json` in the same directory.
///
/// * GIF: every frame of the animation is written.
/// * video: at most [`MAX_EXTRACTED_FRAMES`] frames, sampled evenly over the
///   duration of the clip, are written.  The first frame of the clip is
///   always part of the sample.
///
/// The metadata sidecar is written for every clip; when the clip cannot be
/// decoded (no decoder for the container, a broken file, …) the sidecar only
/// holds the name of the container and a warning is logged.
///
/// When `dry_run` is set, nothing is written: the extraction is only logged.
pub fn extract_video(bytes: &[u8], stem: &str, output_path: &Path, dry_run: bool) -> Extraction {
    let extraction = match video_parser::detect_format(bytes) {
        Some(VideoFormat::Gif) => extract_gif_frames(bytes, stem, output_path, dry_run),
        _ => extract_video_frames(bytes, stem, output_path, dry_run),
    };

    write_metadata(stem, output_path, &extraction.metadata, dry_run);
    extraction
}

/// Extract the frames of a video clip (mp4/webm).
///
/// The frames are sampled evenly over the duration of the clip; see
/// [`MAX_EXTRACTED_FRAMES`].
fn extract_video_frames(bytes: &[u8], stem: &str, output_path: &Path, dry_run: bool) -> Extraction {
    let format = video_parser::detect_format(bytes);
    let format_name = format.as_ref().map(format_name).unwrap_or("unknown");

    let mut player = match VideoPlayer::open(bytes) {
        Ok(player) => player,
        Err(error) => {
            // No decoder for the container (or a broken file): the raw file is
            // all that can be written.
            warn!("cannot decode `{stem}`: {error}; no frame was extracted");
            return Extraction {
                metadata: VideoMetadata {
                    format: format_name.to_string(),
                    ..VideoMetadata::default()
                },
                frames: 0,
            };
        }
    };

    let info = player.info().clone();
    let mut metadata = VideoMetadata::from_info(format_name, &info);
    log_metadata(&metadata);

    // The number of frames that are sampled: the number of frames the clip
    // advertises, capped so that a long video does not produce thousands of
    // PNG files.  A clip that does not advertise any frame still yields its
    // first frame.
    let count = info.frame_count.clamp(1, MAX_EXTRACTED_FRAMES);

    let mut extracted = 0;
    for index in 0..count {
        let time = sample_time(&info, index, count);

        let frame = match player.frame_at(time) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                debug!("no frame at {time}s in `{stem}`");
                continue;
            }
            Err(error) => {
                warn!("cannot decode the frame at {time}s of `{stem}`: {error}");
                continue;
            }
        };

        let path = output_path.with_file_name(format!("{stem}_frame_{index:04}.png"));
        if dry_run {
            debug!(
                "would write the frame at {time}s of `{stem}` to {}",
                path.display()
            );
            extracted += 1;
            continue;
        }

        match write_frame(&path, &frame) {
            Ok(()) => extracted += 1,
            Err(error) => warn!("cannot write the frame at {time}s of `{stem}`: {error}"),
        }
    }

    metadata.extracted_frames = extracted;
    Extraction {
        metadata,
        frames: extracted,
    }
}

/// Extract every frame of a GIF.
///
/// The frames are decoded with the pure Rust GIF backend, so the result does
/// not depend on the `ffmpeg` cargo feature.
fn extract_gif_frames(bytes: &[u8], stem: &str, output_path: &Path, dry_run: bool) -> Extraction {
    let mut metadata = VideoMetadata {
        format: "gif".to_string(),
        ..VideoMetadata::default()
    };

    // The metadata comes from the decoder when it is available.
    match VideoPlayer::open(bytes) {
        Ok(player) => {
            let info = player.info().clone();
            metadata = VideoMetadata::from_info("gif", &info);
        }
        Err(error) => warn!("cannot probe the GIF `{stem}`: {error}"),
    }
    log_metadata(&metadata);

    let Some(frames) = video_parser::save_gif_frames(bytes, stem) else {
        warn!("cannot decode the frames of the GIF `{stem}`");
        return Extraction {
            metadata,
            frames: 0,
        };
    };

    let mut extracted = 0;
    for (frame, suffix) in &frames {
        let path = output_path.with_file_name(suffix);
        if dry_run {
            debug!("would write the frame `{suffix}` of `{stem}`");
            extracted += 1;
            continue;
        }

        match fs::write(&path, frame) {
            Ok(()) => extracted += 1,
            Err(error) => warn!("cannot write the frame `{suffix}` of `{stem}`: {error}"),
        }
    }

    metadata.extracted_frames = extracted;
    Extraction {
        metadata,
        frames: extracted,
    }
}

/// Write the metadata of a clip (`<stem>.video.json`).
fn write_metadata(stem: &str, output_path: &Path, metadata: &VideoMetadata, dry_run: bool) {
    let path = output_path.with_file_name(format!("{stem}.video.json"));

    let json = match serde_json::to_string_pretty(metadata) {
        Ok(json) => json,
        Err(error) => {
            warn!("cannot serialize the metadata of `{stem}`: {error}");
            return;
        }
    };

    if dry_run {
        debug!("would write the metadata of `{stem}` to {}", path.display());
        return;
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = create_dir_all(parent)
    {
        warn!("cannot create {}: {}", parent.display(), error);
        return;
    }

    if let Err(error) = fs::write(&path, json) {
        warn!(
            "cannot write the metadata of `{stem}` to {}: {}",
            path.display(),
            error
        );
    }
}

/// Write a decoded frame as a PNG file.
fn write_frame(path: &Path, frame: &VideoFrame) -> Result<(), String> {
    // The frames of a video are YUV planes: they are converted into RGBA
    // on the CPU, which is what the PNG export needs.
    let pixels = frame.rgba().map_err(|error| error.to_string())?;
    let image =
        RgbaImage::from_raw(frame.width, frame.height, pixels.to_vec()).ok_or_else(|| {
            format!(
                "the {}x{} frame does not hold the expected number of pixels",
                frame.width, frame.height
            )
        })?;

    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    image
        .write_to(&mut writer, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())?;
    Ok(())
}

/// The timestamp of the `index`-th sampled frame of a clip.
///
/// The frames are spread evenly over the duration of the clip; the frame rate
/// is used when the duration is unknown.
fn sample_time(info: &VideoInfo, index: u32, count: u32) -> f64 {
    if info.duration > 0.0 {
        info.duration * f64::from(index) / f64::from(count)
    } else if info.fps > 0.0 {
        f64::from(index) / info.fps
    } else {
        0.0
    }
}

/// The name of a container, as it appears in the metadata sidecar.
fn format_name(format: &VideoFormat) -> &'static str {
    match format {
        VideoFormat::Mp4 => "mp4",
        VideoFormat::WebM => "webm",
        VideoFormat::Gif => "gif",
        VideoFormat::Unknown(_) => "unknown",
    }
}

/// Log the metadata of a clip.
fn log_metadata(metadata: &VideoMetadata) {
    debug!("  Dimensions: {}x{}", metadata.width, metadata.height);
    debug!("  Duration: {}s", metadata.duration);
    debug!("  FPS: {}", metadata.fps);
    debug!("  Frame count: {}", metadata.frame_count);
    debug!("  Codec: {}", metadata.codec);
    debug!("  Pixel format: {}", metadata.pixel_format);
    debug!("  Audio: {}", metadata.has_audio);
}
