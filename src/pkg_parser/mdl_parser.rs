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

/// MDLV data section (control points and triangles).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlvData {
    pub marker_type: u32,
    pub record_block_size: u32,
    pub records: Vec<ControlPoint>,
    pub triangles: Vec<Triangle>,
}

/// A single 80-byte control point record.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPoint {
    pub index: u32,
    pub pos_x: i16,
    pub pos_y: i16,
    pub tex_u: i16,
    pub tex_v: i16,
    pub group_id: u32,
    pub sub_group: u32,
    pub sub_sub_group: u32,
    pub field_0: u32,
    pub field_4: u32,
    pub field_12: u32,
    pub field_16: u32,
    pub field_28: f32,
    pub field_56: f32,
    pub field_60: u32,
    pub field_72: u32,
    pub field_76: f32,
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
    pub unk1: u32,
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
    /// Parse an MDL file from raw bytes using direct byte slicing.
    pub fn new(bytes: &[u8]) -> Option<MdlFile> {
        let mut pos: usize = 0;

        let header = MdlvHeader::parse(bytes, &mut pos)?;
        let data = MdlvData::parse(bytes, &mut pos, header.header_size)?;
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
    fn parse(bytes: &[u8], pos: &mut usize) -> Option<MdlvHeader> {
        // Magic: MDLV0023 (8 bytes)
        let magic = bytes.get(*pos..*pos + 8)?;
        if magic != b"MDLV0023" {
            return None;
        }
        *pos += 8;

        let type_val = read_u32_le(bytes, pos)?;
        let sub_version = read_u16_le(bytes, pos)?;
        let flags = read_u16_le(bytes, pos)?;
        let unknown_16 = read_u32_le(bytes, pos)?;

        // Padding byte at offset 20
        *pos += 1;

        // Material path
        let material_path = read_cstring(bytes, pos)?;

        let header_size = bytes[*pos..]
            .windows(4)
            .position(|w| w == [0x00, 0x0f, 0x00, 0x80])
            .map(|p| *pos + p)
            .unwrap_or(bytes.len());

        Some(MdlvHeader {
            magic: "MDLV0023".to_string(),
            type_val,
            sub_version,
            flags,
            unknown_16,
            material_path,
            header_size,
        })
    }
}

impl MdlvData {
    fn parse(bytes: &[u8], pos: &mut usize, data_start: usize) -> Option<MdlvData> {
        *pos = data_start;

        // Marker: 0x80000F00 (4 bytes)
        let marker_type = read_u32_le(bytes, pos)?;
        if marker_type != 0x80000F00 {
            return None;
        }

        // Skip type byte
        *pos += 1;

        // 3-byte record block size
        let b = bytes.get(*pos..*pos + 3)?;
        *pos += 3;
        let record_block_size = u32::from_le_bytes([b[0], b[1], b[2], 0]);

        // Parse control point records (each 80 bytes)
        let record_start = data_start + 8;
        let record_end = record_start + record_block_size as usize;
        let record_size = 80usize;
        let num_records = record_block_size as usize / record_size;

        let mut records = Vec::with_capacity(num_records);
        for i in 0..num_records {
            let off = record_start + i * record_size;
            if off + record_size > bytes.len() {
                break;
            }
            records.push(ControlPoint::parse(bytes, off, i as u32));
        }

        // Parse remaining data (between records and MDLS) as triangles
        let mdls_pos = bytes[record_end..]
            .windows(4)
            .position(|w| w == b"MDLS")
            .map(|p| record_end + p)
            .unwrap_or(bytes.len());
        let triangles = Self::parse_triangles_from_gap(bytes, record_end, mdls_pos);

        *pos = mdls_pos;

        Some(MdlvData {
            marker_type,
            record_block_size,
            records,
            triangles,
        })
    }

    /// Parse the data between records_end and MDLS as triangles.
    ///
    /// The data starts with a 5-byte header, followed by triangle index
    /// triplets (3 × u16 per triangle).
    fn parse_triangles_from_gap(
        bytes: &[u8], start: usize, mdls_pos: usize,
    ) -> Vec<Triangle> {
        let mut triangles = Vec::new();

        if mdls_pos < start + 5 + 6 {
            return triangles;
        }

        // Skip the 5-byte section header
        let data_start = start + 5;
        let gap_size = mdls_pos - data_start;
        let max_u16 = gap_size / 2;

        let mut i = 0;
        while i + 3 <= max_u16 {
            let off = data_start + i * 2;
            let a = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
            let b = u16::from_le_bytes([bytes[off + 2], bytes[off + 3]]);
            let c = u16::from_le_bytes([bytes[off + 4], bytes[off + 5]]);
            triangles.push(Triangle { a, b, c });
            i += 3;
        }

        triangles
    }
}

impl ControlPoint {
    fn parse(bytes: &[u8], offset: usize, index: u32) -> ControlPoint {
        let field_0 = u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
        let field_4 = u32::from_le_bytes([bytes[offset + 4], bytes[offset + 5], bytes[offset + 6], bytes[offset + 7]]);

        let pos_x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let pos_y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let tex_u = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let tex_v = i16::from_le_bytes([bytes[offset + 6], bytes[offset + 7]]);

        let group_id = u32::from_le_bytes([bytes[offset + 8], bytes[offset + 9], bytes[offset + 10], bytes[offset + 11]]);
        let field_12 = u32::from_le_bytes([bytes[offset + 12], bytes[offset + 13], bytes[offset + 14], bytes[offset + 15]]);
        let field_16 = u32::from_le_bytes([bytes[offset + 16], bytes[offset + 17], bytes[offset + 18], bytes[offset + 19]]);
        let field_28 = f32::from_le_bytes([bytes[offset + 28], bytes[offset + 29], bytes[offset + 30], bytes[offset + 31]]);
        let sub_group = u32::from_le_bytes([bytes[offset + 32], bytes[offset + 33], bytes[offset + 34], bytes[offset + 35]]);
        let sub_sub_group = u32::from_le_bytes([bytes[offset + 64], bytes[offset + 65], bytes[offset + 66], bytes[offset + 67]]);
        let field_56 = f32::from_le_bytes([bytes[offset + 56], bytes[offset + 57], bytes[offset + 58], bytes[offset + 59]]);
        let field_60 = u32::from_le_bytes([bytes[offset + 60], bytes[offset + 61], bytes[offset + 62], bytes[offset + 63]]);
        let field_72 = u32::from_le_bytes([bytes[offset + 72], bytes[offset + 73], bytes[offset + 74], bytes[offset + 75]]);
        let field_76 = f32::from_le_bytes([bytes[offset + 76], bytes[offset + 77], bytes[offset + 78], bytes[offset + 79]]);

        ControlPoint {
            index, pos_x, pos_y, tex_u, tex_v,
            group_id, sub_group, sub_sub_group,
            field_0, field_4, field_12, field_16,
            field_28, field_56, field_60, field_72, field_76,
        }
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
                unk1,
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

        let animation_data = bytes[*pos..].to_vec();
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
