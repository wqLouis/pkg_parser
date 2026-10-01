//! MDLA animation section: clips, per-bone tracks and dense keyframes.
//!
//! A file can contain more than one clip (e.g. `Animation 1`, `eyes`); each
//! clip stores one track per bone and every track samples **every** frame
//! (`frame_count + 1` keyframes of 36 bytes), so playback is a plain
//! lerp-free index into the track.

use serde::Serialize;

use super::reader::{read_cstring, read_f32, read_u32};

/// One keyframe record: 9 little-endian floats (36 bytes).
///
/// Values verified against the bind-pose matrices in MDLS/MDLE: the first
/// frame of a track reproduces the bone's rest transform exactly, with
/// `rz` in radians matching the rotation stored in the matrix.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    /// Translation X.
    pub tx: f32,
    /// Translation Y.
    pub ty: f32,
    /// Translation Z (0 for 2D puppets).
    pub tz: f32,
    /// Rotation around X (radians, ~0 for 2D puppets).
    pub rx: f32,
    /// Rotation around Y (radians, ~0 for 2D puppets).
    pub ry: f32,
    /// Rotation around Z (radians) — the visible 2D rotation.
    pub rz: f32,
    /// Scale X.
    pub sx: f32,
    /// Scale Y.
    pub sy: f32,
    /// Scale Z.
    pub sz: f32,
}

/// Byte size of a serialized [`Keyframe`].
pub const KEYFRAME_BYTES: usize = 36;

/// One animation clip (`"Animation 1"`, `"eyes"`, …).
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnimationClip {
    pub name: String,
    /// Playback mode, e.g. `"loop"`.
    pub loop_mode: String,
    /// Frames per second (7.5 … 60 observed).
    pub fps: f32,
    /// Timeline length in frames; tracks hold `frame_count + 1` samples.
    pub frame_count: u32,
    /// One track per bone, in bone order.
    pub tracks: Vec<Track>,
}

/// Keyframe track for a single bone.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub keyframes: Vec<Keyframe>,
}

/// MDLA (animation) section — optional; files may ship without one.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    /// Section magic, e.g. `MDLA0006`.
    pub header: String,
    /// Absolute offset where the section ends (start of the next section).
    pub end_offset: u32,
    pub num_animations: u32,
    /// Section-header field of unclear meaning (215 … 3330 observed); kept
    /// for compatibility, it is *not* the timeline length.
    pub num_frames: u32,
    /// Name of the first clip (kept for compatibility with older callers).
    pub animation_name: String,
    /// Loop mode of the first clip.
    pub loop_mode: String,
    /// All clips stored in the section.
    pub clips: Vec<AnimationClip>,
}

impl Animation {
    /// Parse the section starting at `offset`; returns the animation and the
    /// absolute offset of the next section.
    pub(super) fn parse(bytes: &[u8], offset: usize) -> Option<(Animation, usize)> {
        let mut pos = offset;
        let header = read_cstring(bytes, &mut pos)?;
        if !header.starts_with("MDLA") {
            return None;
        }

        let end_offset = read_u32(bytes, &mut pos)?;
        let num_animations = read_u32(bytes, &mut pos)?;
        let num_frames = read_u32(bytes, &mut pos)?;
        let _reserved = read_u32(bytes, &mut pos)?;

        if num_animations == 0 || num_animations > 64 {
            return None;
        }
        let limit = if end_offset as usize > offset && end_offset as usize <= bytes.len() {
            end_offset as usize
        } else {
            bytes.len()
        };

        let mut clips = Vec::with_capacity(num_animations as usize);
        for i in 0..num_animations {
            if i > 0 {
                // Clips after the first are preceded by zero padding and a
                // small unknown prefix — probe the three possible positions.
                pos = find_next_clip(bytes, pos, limit)?;
            }
            let Some(clip) = parse_clip(bytes, &mut pos, limit) else {
                return None; // the section doesn't match the layout we know
            };
            clips.push(clip);
        }

        let animation_name = clips.first().map(|c| c.name.clone()).unwrap_or_default();
        let loop_mode = clips.first().map(|c| c.loop_mode.clone()).unwrap_or_default();

        Some((
            Animation {
                header,
                end_offset,
                num_animations,
                num_frames,
                animation_name,
                loop_mode,
                clips,
            },
            limit,
        ))
    }
}

/// Parse one clip at `pos` (no-op and `None` if the bytes don't look like a
/// clip header — `pos` is only advanced on success).
fn parse_clip(bytes: &[u8], pos: &mut usize, limit: usize) -> Option<AnimationClip> {
    let mut p = *pos;
    let name = read_cstring(bytes, &mut p)?;
    let loop_mode = read_cstring(bytes, &mut p)?;
    let fps = read_f32(bytes, &mut p)?;
    let frame_count = read_u32(bytes, &mut p)?;
    let _reserved = read_u32(bytes, &mut p)?;
    let track_count = read_u32(bytes, &mut p)?;

    // Sanity checks — reject padding/garbage instead of allocating wildly.
    if !is_valid_name(&name)
        || !is_valid_tag(&loop_mode)
        || !(0.0..=1000.0).contains(&fps)
        || fps == 0.0
        || frame_count == 0
        || frame_count > 1_000_000
        || track_count == 0
        || track_count > 4096
        || p > limit
    {
        return None;
    }

    let mut tracks = Vec::with_capacity(track_count as usize);
    for _ in 0..track_count {
        let _reserved = read_u32(bytes, &mut p)?; // always 0
        let keyframe_bytes = read_u32(bytes, &mut p)? as usize;
        // Exact size: one dense sample per frame, 36 bytes each — verified
        // on every known file, so anything else is not the layout we know.
        if keyframe_bytes != (frame_count as usize + 1) * KEYFRAME_BYTES {
            return None;
        }
        if p + keyframe_bytes > limit.min(bytes.len()) {
            return None;
        }
        let mut keyframes = Vec::with_capacity(keyframe_bytes / KEYFRAME_BYTES);
        for _ in 0..keyframe_bytes / KEYFRAME_BYTES {
            let mut k = Keyframe::default();
            k.tx = read_f32(bytes, &mut p)?;
            k.ty = read_f32(bytes, &mut p)?;
            k.tz = read_f32(bytes, &mut p)?;
            k.rx = read_f32(bytes, &mut p)?;
            k.ry = read_f32(bytes, &mut p)?;
            k.rz = read_f32(bytes, &mut p)?;
            k.sx = read_f32(bytes, &mut p)?;
            k.sy = read_f32(bytes, &mut p)?;
            k.sz = read_f32(bytes, &mut p)?;
            keyframes.push(k);
        }
        tracks.push(Track { keyframes });
    }

    *pos = p;
    Some(AnimationClip {
        name,
        loop_mode,
        fps,
        frame_count,
        tracks,
    })
}

/// Position of the next clip header after `pos`.
///
/// Clips are separated by zero padding (alignment + a fixed trailer) plus an
/// unknown prefix of at most two `u32`s, so there is no scanning here: the
/// three positions the format can produce are probed and every probe is
/// validated against a full clip header (name, loop mode, fps, frame count,
/// track count, exact keyframe sizes), so a wrong position is never accepted.
fn find_next_clip(bytes: &[u8], mut pos: usize, limit: usize) -> Option<usize> {
    for _ in 0..3 {
        while pos < limit && bytes[pos] == 0 {
            pos += 1;
        }
        if pos >= limit {
            return None;
        }
        let mut probe = pos;
        if parse_clip(bytes, &mut probe, limit).is_some() {
            return Some(pos);
        }
        pos += 4;
    }
    None
}

/// `true` for a plausible clip name: non-empty and free of control
/// characters.  Names are not necessarily ASCII — `"动画 1"` (CJK) shows up
/// in localised wallpapers, so only control bytes are rejected.
fn is_valid_name(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_control)
}

/// `true` for a plausible loop mode (may be empty, must not be garbage).
fn is_valid_tag(s: &str) -> bool {
    !s.chars().any(char::is_control)
}
