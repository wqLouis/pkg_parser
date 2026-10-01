//! MDAT attachment section: named sockets attached to bones.

use serde::Serialize;

use super::reader::{read_cstring, read_f32_array, read_u16, read_u32};

/// MDAT (attachment) section — optional.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Attachments {
    /// Section magic, e.g. `MDAT0001`.
    pub header: String,
    /// Absolute offset of the following section.
    pub next_offset: u32,
    pub entries: Vec<Attachment>,
}

/// One attachment socket.
///
/// ```text
/// u16        bone index the socket is parented to
/// cstring    socket name (e.g. "head", "hair back")
/// f32[16]    4×4 row-major transform, translation in row 3
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub bone_index: u16,
    pub name: String,
    pub transform: [f32; 16],
}

impl Attachments {
    /// Parse the section starting at `offset`; returns the attachment list
    /// and the offset of the next section.
    pub(super) fn parse(bytes: &[u8], offset: usize) -> Option<(Attachments, usize)> {
        let mut pos = offset;
        let header = read_cstring(bytes, &mut pos)?;
        if !header.starts_with("MDAT") {
            return None;
        }

        let next_offset = read_u32(bytes, &mut pos)?;
        let count = read_u16(bytes, &mut pos)?;

        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let bone_index = read_u16(bytes, &mut pos)?;
            let name = read_cstring(bytes, &mut pos)?;
            let transform = read_f32_array::<16>(bytes, &mut pos)?;
            entries.push(Attachment {
                bone_index,
                name,
                transform,
            });
        }

        // The next section starts at `next_offset`; fall back to where the
        // entries ended if the header value is bogus.
        let next = if next_offset as usize >= pos && next_offset as usize <= bytes.len() {
            next_offset as usize
        } else {
            pos
        };

        Some((
            Attachments {
                header,
                next_offset,
                entries,
            },
            next,
        ))
    }
}
