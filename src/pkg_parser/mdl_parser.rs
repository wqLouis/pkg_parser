use serde::Serialize;

// ── Inline byte-slicing helpers ────────────────────────────────────────────

#[inline(always)]
fn read_u32_le(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let b: &[u8; 4] = bytes.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(u32::from_le_bytes(*b))
}

#[inline(always)]
fn read_u16_le(bytes: &[u8], pos: &mut usize) -> Option<u16> {
    let b: &[u8; 2] = bytes.get(*pos..*pos + 2)?.try_into().ok()?;
    *pos += 2;
    Some(u16::from_le_bytes(*b))
}

#[inline(always)]
fn read_f32_le(bytes: &[u8], pos: &mut usize) -> Option<f32> {
    let b: &[u8; 4] = bytes.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(f32::from_le_bytes(*b))
}

/// Read a null-terminated UTF-8 string from a byte slice starting at `offset`.
/// Uses `iter().position()` for fast SIMD-accelerated null-byte search.
/// Advances `offset` past the null terminator.
fn read_cstring(bytes: &[u8], offset: &mut usize) -> Option<String> {
    let remaining = bytes.get(*offset..)?;
    let null_pos = remaining.iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&remaining[..null_pos]).into_owned();
    *offset += null_pos + 1;
    Some(s)
}

/// Parsed MDL (puppet model) file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlFile {
    pub header: MdlvHeader,
    pub data: MdlvData,
    pub bones: Bones,
    pub animation: Animation,
}

/// MDLV section header.
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

/// MDLV data section (control points and triangle indices).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlvData {
    pub records: Vec<ControlPoint>,
    /// Triangle indices into control points (3 × u16 per triangle).
    pub triangles: Vec<Triangle>,
}

/// A single 80-byte control point record.
///
/// Layout (all little-endian):
///   + 0: pos_x  (f32)
///   + 4: pos_y  (f32)
///   + 8..+40:   padding / per-vertex data (mostly unused for static
///               skinning, but reserved for future use)
///   +40: bone_a (u16) — index of bone A
///   +44: bone_b (u16) — index of bone B (0xFFFF means unused)
///   +48..+56:   reserved
///   +56: weight_a (f32) — skinning weight for bone A
///   +60: weight_b (f32) — skinning weight for bone B
///   +64..+72:   reserved
///   +72: tex_u  (f32)
///   +76: tex_v  (f32)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPoint {
    pub index: u32,
    /// Position X (f32; centred around 0, range ≈ [-size/2, +size/2]).
    pub pos_x: f32,
    /// Position Y (f32; centred around 0).
    pub pos_y: f32,
    /// Position Z (always 0 for 2D puppet).
    pub pos_z: f32,
    /// Index of bone A (u16 at offset +40). 0xFFFF if unused.
    pub bone_a: u16,
    /// Index of bone B (u16 at offset +44). 0xFFFF if unused.
    pub bone_b: u16,
    /// Skinning weight for bone A (f32 at offset +56).
    pub weight_a: f32,
    /// Skinning weight for bone B (f32 at offset +60).
    pub weight_b: f32,
    /// Texture U coordinate (f32; range [0, 1]).
    pub tex_u: f32,
    /// Texture V coordinate (f32; range [0, 1]).
    pub tex_v: f32,
}

/// A triangle defined by 3 vertex indices.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Triangle {
    pub a: u16,
    pub b: u16,
    pub c: u16,
}

/// MDLS (bone/skeleton) section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bones {
    pub header: String,
    pub bones: Vec<BoneEntry>,
}

/// A single bone entry with transformation matrix and metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoneEntry {
    pub index: u32,
    pub tmp: u8,
    pub bone_type: u32,
    /// Parent bone index (0xFFFFFFFF means root / no parent).
    /// Forms the bone hierarchy for character-sheet puppets.
    pub parent_index: u32,
    pub matrix: [f32; 16],
    pub info: String,
}

/// MDLA (animation) section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    pub header: String,
    pub end_offset: u32,
    pub num_animations: u32,
    pub num_frames: u32,
    pub animation_name: String,
    pub loop_mode: String,
    pub animation_data: Vec<u8>,
}

impl MdlFile {
    /// Parse an MDL file from raw bytes using the Almamu reference approach.
    pub fn new(bytes: &[u8]) -> Option<MdlFile> {
        let header = MdlvHeader::parse(bytes)?;
        let data = MdlvData::parse(bytes, &header)?;
        // Bones parse starts from MDLS position; we find it from bytes.
        let mut pos = bytes.windows(4).position(|w| w == b"MDLS")?;
        let bones = Bones::parse(bytes, &mut pos)?;
        let animation = Animation::parse(bytes, &mut pos)?;

        Some(MdlFile {
            header,
            data,
            bones,
            animation,
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

impl MdlvHeader {
    fn parse(bytes: &[u8]) -> Option<MdlvHeader> {
        let mut pos: usize = 0;

        // Magic: MDLV0021 or MDLV0023 (8 bytes)
        let magic = bytes.get(pos..pos + 8)?;
        let magic_str = String::from_utf8_lossy(magic).into_owned();
        if magic_str != "MDLV0021" && magic_str != "MDLV0023" {
            return None;
        }
        pos += 8;

        // Padding byte
        pos += 1;

        let type_val = read_u32_le(bytes, &mut pos)?;
        let sub_version = read_u16_le(bytes, &mut pos)?;
        let flags = read_u16_le(bytes, &mut pos)?;
        let unknown_16 = read_u32_le(bytes, &mut pos)?;

        // Material path begins immediately after unknown_16 (at offset 21).
        // There is no separate padding byte — the previous parser dropped the
        // leading 'm' from "materials/..." by reading the cstring at offset 22.
        let material_path = read_cstring(bytes, &mut pos)?;

        Some(MdlvHeader {
            magic: magic_str,
            type_val,
            sub_version,
            flags,
            unknown_16,
            material_path,
            header_size: 0, // unused; kept for backward compat
        })
    }
}

/// Locating the puppet mesh block.
///
/// The mesh block sits at a fully predictable offset from the start of the
/// file once the header is known:
///
///   mesh_offset = 21  (cstring start)
///               + 1   (null terminator)
///               + material_path.len()  (UTF-8 byte length)
///               + 28  (zero padding observed in every normal MDL)
///
/// This sum equals `50 + material_path.len()`. The first 4 bytes of the mesh
/// block are the constant signature `0F 00 80 01`; we verify it as a sanity
/// check and fall back to a byte-level scan only if it doesn't match (e.g.
/// for malformed or skeletonless files that don't follow the standard layout).
const MESH_BLOCK_SIGNATURE: [u8; 4] = [0x0F, 0x00, 0x80, 0x01];
const HEADER_TO_CSTRING_START: usize = 21;
const MESH_PADDING_AFTER_CSTRING: usize = 28;
const MESH_HEADER_SIZE: usize = 8;
const VERTEX_STRIDE: usize = 80;

fn predict_mesh_offset(material_path_len: usize) -> usize {
    HEADER_TO_CSTRING_START + 1 + material_path_len + MESH_PADDING_AFTER_CSTRING
}

/// Find the puppet mesh block at the predicted offset. Returns the block's
/// `(offset, vertex_bytes, index_bytes)` if the layout is the standard one.
///
/// If the predicted offset doesn't have the expected signature, falls back
/// to a byte-level structural scan to handle unusual files.
fn locate_mesh_block(
    bytes: &[u8],
    mdls_offset: usize,
    material_path_len: usize,
) -> Option<(usize, u32, u32)> {
    let search_end = mdls_offset.min(bytes.len());
    let predicted = predict_mesh_offset(material_path_len);

    if let Some(found) = try_read_mesh_block(bytes, predicted, search_end) {
        return Some((predicted, found.0, found.1));
    }

    // Fallback: byte-level structural scan (kept for malformed/unusual files).
    find_mesh_block_scan(bytes, mdls_offset)
}

fn try_read_mesh_block(
    bytes: &[u8],
    block_offset: usize,
    search_end: usize,
) -> Option<(u32, u32)> {
    if block_offset + MESH_HEADER_SIZE > search_end {
        return None;
    }
    if &bytes[block_offset..block_offset + 4] != &MESH_BLOCK_SIGNATURE {
        return None;
    }
    let vertex_bytes = u32::from_le_bytes([
        bytes[block_offset + 4],
        bytes[block_offset + 5],
        bytes[block_offset + 6],
        bytes[block_offset + 7],
    ]);
    if vertex_bytes == 0 || vertex_bytes as usize % VERTEX_STRIDE != 0 {
        return None;
    }
    let vbytes = vertex_bytes as usize;
    if vbytes > bytes.len() {
        return None;
    }
    let vertices_end = block_offset + MESH_HEADER_SIZE + vbytes;
    if vertices_end + 4 > search_end {
        return None;
    }
    let index_bytes = u32::from_le_bytes([
        bytes[vertices_end],
        bytes[vertices_end + 1],
        bytes[vertices_end + 2],
        bytes[vertices_end + 3],
    ]);
    if index_bytes == 0 || index_bytes as usize % 6 != 0 {
        return None;
    }
    let indices_end = vertices_end + 4 + index_bytes as usize;
    if indices_end > search_end {
        return None;
    }
    Some((vertex_bytes, index_bytes))
}

/// Byte-level structural scan, used as a fallback when the predicted offset
/// doesn't match the expected signature. Scans from after the magic+pad to
/// the MDLS section, looking for a block where:
///   - offset+4 contains vertexBytes (multiple of 80)
///   - offset+8+vertexBytes contains indexBytes (multiple of 6)
///   - everything fits before MDLS
fn find_mesh_block_scan(bytes: &[u8], mdls_offset: usize) -> Option<(usize, u32, u32)> {
    const MARKER_SIZE: usize = 9;
    let search_end = mdls_offset.min(bytes.len());
    let mut offset = MARKER_SIZE;

    while offset + MESH_HEADER_SIZE + 4 <= search_end {
        let vertex_bytes = u32::from_le_bytes([
            bytes[offset + 4], bytes[offset + 5], bytes[offset + 6], bytes[offset + 7],
        ]);

        if vertex_bytes == 0 || vertex_bytes as usize % VERTEX_STRIDE != 0 {
            offset += 1;
            continue;
        }

        let vbytes = vertex_bytes as usize;
        if vbytes > bytes.len() {
            offset += 1;
            continue;
        }

        let vertices_end = offset + MESH_HEADER_SIZE + vbytes;
        if vertices_end + 4 > search_end {
            offset += 1;
            continue;
        }

        let index_bytes = u32::from_le_bytes([
            bytes[vertices_end], bytes[vertices_end + 1],
            bytes[vertices_end + 2], bytes[vertices_end + 3],
        ]);

        if index_bytes == 0 || index_bytes as usize % 6 != 0 {
            offset += 1;
            continue;
        }

        let indices_end = vertices_end + 4 + index_bytes as usize;
        if indices_end > search_end {
            offset += 1;
            continue;
        }

        return Some((offset, vertex_bytes, index_bytes));
    }

    None
}

impl MdlvData {
    fn parse(bytes: &[u8], header: &MdlvHeader) -> Option<MdlvData> {
        // Find MDLS marker
        let mdls_offset = bytes.windows(4).position(|w| w == b"MDLS")?;

        // Locate the mesh block at the predicted offset (or fall back to scan)
        let (block_offset, vertex_bytes, index_bytes) =
            locate_mesh_block(bytes, mdls_offset, header.material_path.len())?;

        let vertices_offset = block_offset + MESH_HEADER_SIZE;
        let indices_offset = vertices_offset + vertex_bytes as usize + 4; // +4 for indexBytes field

        let num_records = vertex_bytes as usize / VERTEX_STRIDE;
        let num_indices = index_bytes as usize / 2; // u16

        // Parse control points (see ControlPoint doc for layout).
        let mut records = Vec::with_capacity(num_records);
        for i in 0..num_records {
            let off = vertices_offset + i * VERTEX_STRIDE;
            if off + 80 > bytes.len() {
                break;
            }
            let pos_x = f32::from_le_bytes([bytes[off], bytes[off+1], bytes[off+2], bytes[off+3]]);
            let pos_y = f32::from_le_bytes([bytes[off+4], bytes[off+5], bytes[off+6], bytes[off+7]]);
            let pos_z = f32::from_le_bytes([bytes[off+8], bytes[off+9], bytes[off+10], bytes[off+11]]);
            let bone_a = u16::from_le_bytes([bytes[off+40], bytes[off+41]]);
            let bone_b = u16::from_le_bytes([bytes[off+44], bytes[off+45]]);
            let weight_a = f32::from_le_bytes([bytes[off+56], bytes[off+57], bytes[off+58], bytes[off+59]]);
            let weight_b = f32::from_le_bytes([bytes[off+60], bytes[off+61], bytes[off+62], bytes[off+63]]);
            let tex_u = f32::from_le_bytes([bytes[off+72], bytes[off+73], bytes[off+74], bytes[off+75]]);
            let tex_v = f32::from_le_bytes([bytes[off+76], bytes[off+77], bytes[off+78], bytes[off+79]]);
            records.push(ControlPoint {
                index: i as u32,
                pos_x, pos_y, pos_z,
                bone_a, bone_b,
                weight_a, weight_b,
                tex_u, tex_v,
            });
        }

        // Parse triangle indices (u16, 3 per triangle)
        let mut triangles = Vec::with_capacity(num_indices / 3);
        let mut off = indices_offset;
        while off + 6 <= indices_offset + index_bytes as usize {
            let a = u16::from_le_bytes([bytes[off], bytes[off+1]]);
            let b = u16::from_le_bytes([bytes[off+2], bytes[off+3]]);
            let c = u16::from_le_bytes([bytes[off+4], bytes[off+5]]);
            if (a as usize) < num_records && (b as usize) < num_records && (c as usize) < num_records {
                triangles.push(Triangle { a, b, c });
            }
            off += 6;
        }

        Some(MdlvData { records, triangles })
    }
}

impl Bones {
    fn parse(bytes: &[u8], pos: &mut usize) -> Option<Bones> {
        let header = read_cstring(bytes, pos)?;

        let _next_offset = read_u32_le(bytes, pos)?;
        let num_bones = read_u32_le(bytes, pos)?;

        let mut bones = Vec::with_capacity(num_bones as usize);
        for i in 0..num_bones {
            let tmp = *bytes.get(*pos)?;
            *pos += 1;
            let bone_type = read_u32_le(bytes, pos)?;
            let unk1 = read_u32_le(bytes, pos)?;
            let entry_byte_len = read_u32_le(bytes, pos)?;

            let num_floats = entry_byte_len as usize / 4;
            let mut matrix = [0.0f32; 16];
            for j in 0..num_floats.min(16) {
                let f = read_f32_le(bytes, pos)?;
                matrix[j] = f;
            }
            if num_floats > 16 {
                *pos += (num_floats - 16) * 4;
            }

            let info = read_cstring(bytes, pos)?;

            bones.push(BoneEntry {
                index: i,
                tmp,
                bone_type,
                parent_index: unk1,
                matrix,
                info,
            });
        }

        Some(Bones { header, bones })
    }
}

impl Animation {
    fn parse(bytes: &[u8], pos: &mut usize) -> Option<Animation> {
        let header = read_cstring(bytes, pos)?;

        let end_offset = read_u32_le(bytes, pos)?;
        let num_animations = read_u32_le(bytes, pos)?;
        let num_frames = read_u32_le(bytes, pos)?;
        let _unk = read_u32_le(bytes, pos)?;

        let animation_name = read_cstring(bytes, pos).unwrap_or_default();
        let loop_mode = read_cstring(bytes, pos).unwrap_or_default();

        let animation_data = bytes.get(*pos..).unwrap_or(&[]).to_vec();
        *pos = bytes.len();

        Some(Animation {
            header,
            end_offset,
            num_animations,
            num_frames,
            animation_name,
            loop_mode,
            animation_data,
        })
    }
}
