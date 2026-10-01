//! MDLS skeleton section: bone hierarchy and bind-pose transforms.

use serde::Serialize;

use super::reader::{read_cstring, read_f32, read_u32, read_u8};

/// Upper bound for a single bone payload; anything bigger means the parse
/// drifted and the section should be rejected instead of allocating.
const MAX_BONE_BYTES: u32 = 4096;

/// MDLS (bone/skeleton) section.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Bones {
    /// Section magic, e.g. `MDLS0004`.
    pub header: String,
    /// Absolute offset of the section that follows MDLS (MDAT/MDLA/...).
    pub next_offset: u32,
    pub bones: Vec<BoneEntry>,
    /// Undecoded bytes between the end of the bone list and the next
    /// section.  They hold per-bone deform/keyframe metadata that is not
    /// understood yet; kept raw so no data is lost.
    pub trailing: Vec<u8>,
}

/// A single bone entry: transform plus JSON metadata and a name.
///
/// ```text
/// u32            bone type (0 = root-ish, 1 = regular bone observed)
/// u32            parent index (0xFFFFFFFF = no parent / root)
/// u32            payload byte length (64 = a 4×4 f32 matrix)
/// [payload]      bind-pose 4×4 matrix, row-major, translation in row 3
/// cstring        info JSON (e.g. {"tp":"36.1 726.8 0","tm":100.0})
/// cstring        bone name (e.g. "legs", often empty)
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoneEntry {
    pub index: u32,
    pub bone_type: u32,
    /// Parent bone index (`0xFFFFFFFF` means root / no parent).
    pub parent_index: u32,
    /// Bind-pose 4×4 matrix (row-major; row 3 is the translation).
    pub matrix: [f32; 16],
    /// JSON metadata — `tp` is the bone pin position, `tm` a multiplier.
    pub info: String,
    /// Bone name (empty in many files).
    pub name: String,
}

impl Bones {
    /// Parse the section starting at `offset`.  Returns the bones and the
    /// absolute offset of the next section.
    ///
    /// `None` means the section is malformed — callers should fall back to
    /// scanning for the next known section marker.
    pub(super) fn parse(bytes: &[u8], offset: usize) -> Option<(Bones, usize)> {
        let mut pos = offset;
        let header = read_cstring(bytes, &mut pos)?;
        if !header.starts_with("MDLS") {
            return None;
        }

        let next_offset = read_u32(bytes, &mut pos)?;
        let num_bones = read_u32(bytes, &mut pos)?;
        // Reserved byte between the count and the first bone (always 0).
        let _reserved = read_u8(bytes, &mut pos)?;

        if num_bones as usize > bytes.len() {
            return None;
        }

        let mut bones = Vec::with_capacity(num_bones as usize);
        for i in 0..num_bones {
            let bone_type = read_u32(bytes, &mut pos)?;
            let parent_index = read_u32(bytes, &mut pos)?;
            let byte_len = read_u32(bytes, &mut pos)?;
            if byte_len == 0 || byte_len > MAX_BONE_BYTES {
                return None;
            }

            // The payload is a 4×4 matrix today; tolerate other sizes by
            // keeping the first 16 floats and skipping the rest.
            let mut matrix = [0.0f32; 16];
            let num_floats = (byte_len / 4) as usize;
            for slot in matrix.iter_mut().take(num_floats.min(16)) {
                *slot = read_f32(bytes, &mut pos)?;
            }
            let skipped = (num_floats.saturating_sub(16)) * 4;
            bytes.get(pos..pos + skipped)?;
            pos += skipped;

            let info = read_cstring(bytes, &mut pos)?;
            let name = read_cstring(bytes, &mut pos)?;

            bones.push(BoneEntry {
                index: i,
                bone_type,
                parent_index,
                matrix,
                info,
                name,
            });
        }

        // Everything up to the next section is undecoded metadata.
        let next = if next_offset as usize >= pos && next_offset as usize <= bytes.len() {
            next_offset as usize
        } else {
            pos
        };
        let trailing = bytes[pos..next].to_vec();

        Some((
            Bones {
                header,
                next_offset,
                bones,
                trailing,
            },
            next,
        ))
    }
}
