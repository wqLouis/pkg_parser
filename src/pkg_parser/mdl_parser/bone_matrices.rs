//! MDLE section: per-bone 4×4 matrices.
//!
//! Present in some files only (right after the last animation section).  The
//! payload is a flat list of row-major 4×4 transforms — one per bone, in
//! bone order — with the translation in row 3.

use serde::Serialize;

use super::reader::{read_cstring, read_f32_array, read_u32};

/// MDLE (bone matrix) section — optional.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoneMatrices {
    /// Section magic, e.g. `MDLE0002`.
    pub header: String,
    /// Absolute offset where the section ends.
    pub end_offset: u32,
    /// One 4×4 matrix per bone (row-major, translation in row 3).
    pub matrices: Vec<[f32; 16]>,
}

impl BoneMatrices {
    /// Parse the section starting at `offset`; returns the matrices and the
    /// absolute offset of the next section.
    pub(super) fn parse(bytes: &[u8], offset: usize) -> Option<(BoneMatrices, usize)> {
        let mut pos = offset;
        let header = read_cstring(bytes, &mut pos)?;
        if !header.starts_with("MDLE") {
            return None;
        }

        let end_offset = read_u32(bytes, &mut pos)?;
        let data_len = read_u32(bytes, &mut pos)? as usize;
        if data_len % 64 != 0 || pos + data_len > bytes.len() {
            return None;
        }

        let mut matrices = Vec::with_capacity(data_len / 64);
        let mut p = pos;
        while p < pos + data_len {
            matrices.push(read_f32_array::<16>(bytes, &mut p)?);
        }

        // The section ends at `end_offset`; when it is the last section the
        // value points at the file's trailing sentinel byte.
        let next = if end_offset as usize > offset && end_offset as usize <= bytes.len() {
            end_offset as usize
        } else {
            pos + data_len
        };

        Some((
            BoneMatrices {
                header,
                end_offset,
                matrices,
            },
            next,
        ))
    }
}
