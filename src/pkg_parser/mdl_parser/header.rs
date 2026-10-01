//! MDLV file header.

use serde::Serialize;

use super::reader::{read_cstring, read_u16, read_u32};

/// MDLV section header.
///
/// Layout (little-endian), the string starts at offset `0x15`:
///
/// | Off | Size | Field |
/// |-----|------|-------|
/// | 0x00 | 8 | magic: `MDLV0021` / `MDLV0023` |
/// | 0x08 | 1 | reserved (0) |
/// | 0x09 | 4 | type word `0x01800009` — same family as the mesh block tag `0x0180000F` |
/// | 0x0D | 2 | sub-version (1 observed) |
/// | 0x0F | 2 | flags (0 observed) |
/// | 0x11 | 4 | unknown (0 observed) |
/// | 0x15 | var | material path, null-terminated |
///
/// Note: the header fields can also be read as `u32@0x08 = 0x80000900`,
/// `u16@0x0C = 0x0101`, `u16 = 0`, `u32@0x10 = 256`, `u8` pad — both readings
/// consume exactly 21 bytes.  The layout above is kept because the type word
/// then matches the mesh block tag (`0x01800009` vs `0x0180000F`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlvHeader {
    pub magic: String,
    pub type_val: u32,
    pub sub_version: u16,
    pub flags: u16,
    pub unknown_16: u32,
    pub material_path: String,
    pub header_size: usize,
}

impl MdlvHeader {
    pub(super) fn parse(bytes: &[u8]) -> Option<MdlvHeader> {
        let mut pos: usize = 0;

        // Magic: MDLV0021 or MDLV0023 (8 bytes)
        let magic = bytes.get(pos..pos + 8)?;
        let magic_str = String::from_utf8_lossy(magic).into_owned();
        if magic_str != "MDLV0021" && magic_str != "MDLV0023" {
            return None;
        }
        pos += 8;

        // Reserved byte
        pos += 1;

        let type_val = read_u32(bytes, &mut pos)?;
        let sub_version = read_u16(bytes, &mut pos)?;
        let flags = read_u16(bytes, &mut pos)?;
        let unknown_16 = read_u32(bytes, &mut pos)?;

        // Material path begins immediately after unknown_16 (at offset 21).
        let material_path = read_cstring(bytes, &mut pos)?;

        Some(MdlvHeader {
            magic: magic_str,
            type_val,
            sub_version,
            flags,
            unknown_16,
            material_path,
            header_size: pos,
        })
    }

    /// Offset of the mesh block: the header ends at `header_size` and is
    /// followed by exactly 28 bytes of zero padding (all known files).
    ///
    /// This is derived, never searched for — the tag is verified there.
    pub(super) fn mesh_offset(&self) -> usize {
        self.header_size + 28
    }
}
