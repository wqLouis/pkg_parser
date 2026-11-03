//! A tiny MP4 header parser.
//!
//! The parser answers the two questions a renderer asks before it can
//! display a video — *how big is it* and *how long is it* — **without
//! decoding anything**.
//!
//! It only walks the box tree of the `moov` box:
//!
//! ```text
//! moov
//!  └─ trak
//!     ├─ tkhd   → the display size of the track (16.16 fixed point)
//!     └─ mdia
//!         ├─ hdlr → the type of the track (`vide`, `soun`, …)
//!         └─ mdhd → the timescale and the duration of the track
//! ```
//!
//! Because only the header is read, the parser is fast, it does not
//! allocate per sample and it works on a file that is not complete yet, as
//! soon as its `moov` box is there.
//!
//! The module is pure Rust: it is compiled for every target, which makes
//! it testable on the host and usable from the browser build (where the
//! media element only knows the size of the video once its metadata was
//! loaded).

/// The metadata of an MP4 file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mp4Meta {
    /// Display width in pixels (from the track header).
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Duration of the track in seconds.
    pub duration: f64,
}

/// The four character code of the `moov` box.
const MOOV: [u8; 4] = *b"moov";
/// The four character code of the `trak` box.
const TRAK: [u8; 4] = *b"trak";
/// The four character code of the `tkhd` box.
const TKHD: [u8; 4] = *b"tkhd";
/// The four character code of the `mdia` box.
const MDIA: [u8; 4] = *b"mdia";
/// The four character code of the `mdhd` box.
const MDHD: [u8; 4] = *b"mdhd";
/// The four character code of the `hdlr` box.
const HDLR: [u8; 4] = *b"hdlr";

/// The handler type of a video track.
const HANDLER_VIDEO: [u8; 4] = *b"vide";

/// Read the metadata of an MP4 file, if it is one.
///
/// The first video track of the file is used.  Returns `None` when the
/// bytes are not an MP4, when they hold no `moov` box or when the track
/// header carries no usable size.
pub fn probe_mp4(bytes: &[u8]) -> Option<Mp4Meta> {
    // The `moov` box can be anywhere in the file: it is usually at the
    // end (after the media data), but a streaming file puts it first.
    let moov = children(bytes).find(|entry| entry.kind == MOOV)?;

    for trak in children(moov.payload) {
        if trak.kind != TRAK {
            continue;
        }

        // The media box holds the handler and the timing of the track.
        let Some(mdia) = find_child(trak.payload, &MDIA) else {
            continue;
        };

        // The handler tells the media type of the track: only the video
        // ones are of interest.  A track without a handler box is assumed
        // to be a video track, because some muxers do not write one.
        if let Some(handler) = find_child(mdia.payload, &HDLR) {
            let is_video = handler.payload.len() >= 12 && handler.payload[8..12] == HANDLER_VIDEO;
            if !is_video {
                continue;
            }
        }

        // The track header carries the display size of the track.
        let Some(tkhd) = find_child(trak.payload, &TKHD) else {
            continue;
        };
        let Some((width, height)) = track_size(&tkhd) else {
            continue;
        };

        // The media header carries the timescale and the duration of the
        // track; both are needed to turn the duration into seconds.
        let duration = find_child(mdia.payload, &MDHD)
            .and_then(|mdhd| track_duration(&mdhd))
            .unwrap_or(0.0);

        return Some(Mp4Meta {
            width,
            height,
            duration,
        });
    }

    None
}

/// A box of the MP4 tree: its type and its payload.
struct Mp4Box<'a> {
    /// The four character code of the box.
    kind: [u8; 4],
    /// The bytes of the box, after its header.
    payload: &'a [u8],
}

/// Read the box that starts at `offset`.
///
/// Returns the box and the offset of the next one, or `None` when the
/// bytes are truncated or when the size of the box is bogus.
fn read_box(bytes: &[u8], offset: usize) -> Option<(Mp4Box<'_>, usize)> {
    let header = bytes.get(offset..offset.checked_add(8)?)?;
    let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let kind = [header[4], header[5], header[6], header[7]];

    let (size, header_size) = match size {
        // A size of one means that the real size is stored in the 64 bit
        // field that follows the type of the box.
        1 => {
            let large = bytes.get(offset.checked_add(8)?..offset.checked_add(16)?)?;
            let size = u64::from_be_bytes(large.try_into().ok()?);
            (usize::try_from(size).ok()?, 16)
        }
        // A size of zero means that the box extends to the end of the file.
        0 => (bytes.len().checked_sub(offset)?, 8),
        size => (size, 8),
    };

    // The box has to hold at least its own header and fit in the bytes.
    if size < header_size {
        return None;
    }
    let end = offset.checked_add(size)?;
    if end > bytes.len() {
        return None;
    }

    Some((
        Mp4Box {
            kind,
            payload: &bytes[offset + header_size..end],
        },
        end,
    ))
}

/// Iterate over the boxes stored in `payload`.
fn children(payload: &[u8]) -> impl Iterator<Item = Mp4Box<'_>> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if offset >= payload.len() {
            return None;
        }
        let (entry, next) = read_box(payload, offset)?;
        offset = next;
        Some(entry)
    })
}

/// Find the first child box of the given type.
fn find_child<'a>(payload: &'a [u8], kind: &[u8; 4]) -> Option<Mp4Box<'a>> {
    children(payload).find(|entry| &entry.kind == kind)
}

/// Read the display size out of a track header box.
///
/// The width and the height are the last two fields of the box; they are
/// 16.16 fixed point numbers (the integer part is the number of pixels).
fn track_size(tkhd: &Mp4Box<'_>) -> Option<(u32, u32)> {
    let payload = tkhd.payload;
    if payload.len() < 8 {
        return None;
    }

    let width = u32::from_be_bytes(
        payload[payload.len() - 8..payload.len() - 4]
            .try_into()
            .ok()?,
    );
    let height = u32::from_be_bytes(payload[payload.len() - 4..].try_into().ok()?);
    let width = width >> 16;
    let height = height >> 16;

    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// Read the duration of a track, in seconds, out of its media header box.
///
/// The media header holds the timescale of the track (the number of time
/// units per second) and its duration, expressed in those units.
fn track_duration(mdhd: &Mp4Box<'_>) -> Option<f64> {
    let payload = mdhd.payload;
    let version = *payload.first()?;

    // Version 0: version/flags (4), creation (4), modification (4),
    //             timescale (4), duration (4).
    // Version 1: version/flags (4), creation (8), modification (8),
    //             timescale (4), duration (8).
    let (timescale, duration) = match version {
        0 => {
            let timescale = u32::from_be_bytes(payload.get(12..16)?.try_into().ok()?);
            let duration = u32::from_be_bytes(payload.get(16..20)?.try_into().ok()?);
            (timescale, u64::from(duration))
        }
        1 => {
            let timescale = u32::from_be_bytes(payload.get(20..24)?.try_into().ok()?);
            let duration = u64::from_be_bytes(payload.get(24..32)?.try_into().ok()?);
            (timescale, duration)
        }
        _ => return None,
    };

    if timescale == 0 {
        return Some(0.0);
    }
    Some(duration as f64 / f64::from(timescale))
}
