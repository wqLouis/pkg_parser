//! `.mdl` puppet model parser.
//!
//! An MDL file is a sequence of null-terminated sections:
//!
//! ```text
//! MDLV…  header + mesh block (+ secondary positions + index batches)
//! MDLS…  skeleton: bone hierarchy, bind-pose matrices, metadata
//! MDAT…  attachments: named sockets parented to bones      (optional)
//! MDLA…  animation: clips → per-bone tracks → keyframes     (optional)
//! MDLE…  per-bone 4×4 matrices                             (optional)
//! \0     end-of-file sentinel (single null byte)
//! ```
//!
//! Every offset is derived from the format itself: the mesh block starts
//! where the header says, its trailer ends where the next section starts,
//! and each section carries the offset of the one after it.  Nothing is
//! searched for — an unknown or malformed section ends the walk instead of
//! scanning data bytes for a marker.  Only the header and the mesh are
//! required for [`MdlFile::new`] to succeed; a model without a skeleton or
//! animation (both may be absent) still renders.

use serde::Serialize;

mod animation;
mod attachments;
mod bone_matrices;
mod header;
mod mesh;
mod reader;
mod skeleton;

pub use animation::{Animation, AnimationClip, Keyframe, Track, KEYFRAME_BYTES};
pub use attachments::{Attachment, Attachments};
pub use bone_matrices::BoneMatrices;
pub use header::MdlvHeader;
pub use mesh::{ControlPoint, IndexBatch, MdlvData, Triangle};
pub use skeleton::{BoneEntry, Bones};

/// Parsed MDL (puppet model) file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlFile {
    pub header: MdlvHeader,
    pub data: MdlvData,
    pub bones: Bones,
    pub attachments: Attachments,
    pub animation: Animation,
    pub bone_matrices: BoneMatrices,
}

impl MdlFile {
    /// Parse an MDL file from raw bytes.
    ///
    /// Nothing is scanned or guessed: the mesh block must sit exactly where
    /// the header says and every section body must match the known layout.
    /// Returns `None` otherwise (bad magic, broken mesh, malformed section).
    /// Skeleton/attachments/animation/matrices are optional — a file that
    /// simply doesn't carry them parses fine without them.
    pub fn new(bytes: &[u8]) -> Option<MdlFile> {
        let header = MdlvHeader::parse(bytes)?;
        let (data, mut pos) = MdlvData::parse(bytes, &header)?;

        // Walk the remaining sections.  Each one carries the offset of the
        // next, so positions are always exact — and everything after the
        // mesh is optional (files without a skeleton exist).
        let mut bones = Bones::default();
        let mut attachments = Attachments::default();
        let mut animation = Animation::default();
        let mut bone_matrices = BoneMatrices::default();

        while pos < bytes.len() {
            if bytes[pos] == 0 {
                break; // end-of-file sentinel
            }
            let magic = bytes.get(pos..pos + 4)?; // truncated section header
            let next = if magic.starts_with(b"MDLS") {
                Bones::parse(bytes, pos).map(|(parsed, next)| {
                    bones = parsed;
                    next
                })
            } else if magic.starts_with(b"MDAT") {
                Attachments::parse(bytes, pos).map(|(parsed, next)| {
                    attachments = parsed;
                    next
                })
            } else if magic.starts_with(b"MDLA") {
                Animation::parse(bytes, pos).map(|(parsed, next)| {
                    animation = parsed;
                    next
                })
            } else if magic.starts_with(b"MDLE") {
                BoneMatrices::parse(bytes, pos).map(|(parsed, next)| {
                    bone_matrices = parsed;
                    next
                })
            } else {
                // Unknown section: stop here rather than guessing where the
                // next one could be.
                log::warn!("mdl: unrecognized section at {pos}, stopping parse");
                break;
            };

            match next {
                Some(next) if next > pos => pos = next,
                Some(_) => return None, // section reported a bogus offset
                None => return None,    // known section, broken body
            }
        }

        Some(MdlFile {
            header,
            data,
            bones,
            attachments,
            animation,
            bone_matrices,
        })
    }

    /// Serialize to pretty-printed JSON.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Serialize to compact JSON.
    pub fn to_json_compact(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}
