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

/// MDLV data section (control points, quads, and render triangles).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlvData {
    pub marker_type: u32,
    pub record_block_size: u32,
    pub records: Vec<ControlPoint>,
    /// Quad-derived triangles indexing into control points (vertex IDs < num_records).
    pub quads: Vec<Triangle>,
    /// Render triangles derived from tessellating quads (vertex IDs may exceed num_records).
    pub triangles: Vec<Triangle>,
}

/// A single 80-byte control point record.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPoint {
    pub index: u32,
    /// Normalized position X (raw i16 / 32767 → range ≈ [-1, 1])
    pub pos_x: f32,
    /// Normalized position Y (raw i16 / 32767 → range ≈ [-1, 1])
    pub pos_y: f32,
    /// Normalized texture U coordinate (raw i16 / 32767 → range ≈ [-1, 1])
    pub tex_u: f32,
    /// Normalized texture V coordinate (raw i16 / 32767 → range ≈ [-1, 1])
    pub tex_v: f32,
    /// Group identifier (shared by control points of the same mesh part).
    pub group_id: u32,
    /// Sub-group index.
    pub sub_group: u32,
    /// Sub-sub-group index.
    pub sub_sub_group: u32,
    /// Deformation parameter / weight (only ~14 unique values across all points).
    pub weight: f32,
    /// Normalized deformation handle A, X component.
    pub handle_a_x: f32,
    /// Normalized deformation handle A, Y component.
    pub handle_a_y: f32,
    /// Normalized deformation handle B, X component.
    pub handle_b_x: f32,
    /// Normalized deformation handle B, Y component.
    pub handle_b_y: f32,
    /// Normalized deformation handle C, X component.
    pub handle_c_x: f32,
    /// Normalized deformation handle C, Y component.
    pub handle_c_y: f32,
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
        // Guard against bogus data_start (must not exceed the buffer)
        if data_start + 8 > bytes.len() {
            return None;
        }

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
        let record_size = 80usize;
        let num_records = record_block_size as usize / record_size;

        // Sanity checks to avoid overflow / huge allocations on malformed data
        let Some(record_end) = record_start.checked_add(record_block_size as usize) else {
            return None;
        };
        if record_end > bytes.len() || num_records > bytes.len() / record_size {
            return None;
        }

        let mut records = Vec::with_capacity(num_records);
        for i in 0..num_records {
            let off = record_start + i * record_size;
            if off + record_size > bytes.len() {
                break;
            }
            records.push(ControlPoint::parse(bytes, off, i as u32));
        }

        // Parse remaining data (between records and MDLS) as triangles.
        // Use .get() to avoid panicking if record_end is out of bounds.
        let mdls_pos = bytes
            .get(record_end..)
            .and_then(|tail| {
                tail.windows(4)
                    .position(|w| w == b"MDLS")
                    .map(|p| record_end + p)
            })
            .unwrap_or(bytes.len());
        let (quads, triangles) = Self::parse_triangles_from_gap(bytes, record_end, mdls_pos);

        *pos = mdls_pos;

        Some(MdlvData {
            marker_type,
            record_block_size,
            records,
            quads,
            triangles,
        })
    }

    /// Parse the data between records_end and MDLS into two sections.
    ///
    /// The gap starts with a 5-byte header:
    ///   byte[0]      — section type (0x3e or 0x3f)
    ///   bytes[1..5]  — u32 LE: size in bytes of the quads triangle data
    ///
    /// After the quads data, the remaining bytes are the render triangles
    /// section (derived from tessellating the quads; vertex indices may far
    /// exceed the control point count).
    fn parse_triangles_from_gap(
        bytes: &[u8], start: usize, mdls_pos: usize,
    ) -> (Vec<Triangle>, Vec<Triangle>) {
        let mut quads = Vec::new();
        let mut triangles = Vec::new();

        let mdls_pos = mdls_pos.min(bytes.len());
        if mdls_pos < start + 5 + 6 {
            return (quads, triangles);
        }

        // Read 5-byte section header
        let header = &bytes[start..start + 5];
        let quads_data_size = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;

        let data_start = start + 5;
        let gap_end = mdls_pos;

        // Clamp quads_data_size to available data
        let quads_end = data_start.saturating_add(quads_data_size).min(gap_end);

        // Parse quads section
        let mut off = data_start;
        while off + 6 <= quads_end {
            let Some(chunk) = bytes.get(off..off + 6) else {
                break;
            };
            quads.push(Triangle {
                a: u16::from_le_bytes([chunk[0], chunk[1]]),
                b: u16::from_le_bytes([chunk[2], chunk[3]]),
                c: u16::from_le_bytes([chunk[4], chunk[5]]),
            });
            off += 6;
        }

        // Parse render triangles section (remainder of gap)
        off = quads_end;
        while off + 6 <= gap_end {
            let Some(chunk) = bytes.get(off..off + 6) else {
                break;
            };
            triangles.push(Triangle {
                a: u16::from_le_bytes([chunk[0], chunk[1]]),
                b: u16::from_le_bytes([chunk[2], chunk[3]]),
                c: u16::from_le_bytes([chunk[4], chunk[5]]),
            });
            off += 6;
        }

        (quads, triangles)
    }
}

impl ControlPoint {
    fn parse(bytes: &[u8], offset: usize, index: u32) -> ControlPoint {
        let pos_x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let pos_y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let tex_u = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let tex_v = i16::from_le_bytes([bytes[offset + 6], bytes[offset + 7]]);
        let group_id = u32::from_le_bytes([bytes[offset + 8], bytes[offset + 9], bytes[offset + 10], bytes[offset + 11]]);
        // bytes 12–27: padding / markers (always 0 or constant 0x80000000)
        let handle_a_x = i16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]);
        let handle_a_y = i16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]);
        let sub_group = u32::from_le_bytes([bytes[offset + 32], bytes[offset + 33], bytes[offset + 34], bytes[offset + 35]]);
        // bytes 36–39: marker (always 0x80000000)
        let weight = i16::from_le_bytes([bytes[offset + 40], bytes[offset + 41]]);
        // bytes 42–55: padding (always 0)
        // bytes 56–59: marker (always 0x80000000)
        // bytes 60–63: constant marker (always 0x3f000000 = 63)
        let sub_sub_group = u32::from_le_bytes([bytes[offset + 64], bytes[offset + 65], bytes[offset + 66], bytes[offset + 67]]);
        // bytes 68–71: padding (always 0)
        let handle_b_x = i16::from_le_bytes([bytes[offset + 72], bytes[offset + 73]]);
        let handle_b_y = i16::from_le_bytes([bytes[offset + 74], bytes[offset + 75]]);
        let handle_c_x = i16::from_le_bytes([bytes[offset + 76], bytes[offset + 77]]);
        let handle_c_y = i16::from_le_bytes([bytes[offset + 78], bytes[offset + 79]]);

        // Normalize i16 coords to f32 range ≈ [-1, 1]
        const NORM: f32 = 32767.0;
        let pos_x = pos_x as f32 / NORM;
        let pos_y = pos_y as f32 / NORM;
        let tex_u = tex_u as f32 / NORM;
        let tex_v = tex_v as f32 / NORM;
        let handle_a_x = handle_a_x as f32 / NORM;
        let handle_a_y = handle_a_y as f32 / NORM;
        let handle_b_x = handle_b_x as f32 / NORM;
        let handle_b_y = handle_b_y as f32 / NORM;
        let handle_c_x = handle_c_x as f32 / NORM;
        let handle_c_y = handle_c_y as f32 / NORM;
        // weight is a small positive parameter, normalize to [0, ~0.1]
        let weight = weight as f32 / NORM;

        ControlPoint {
            index, pos_x, pos_y, tex_u, tex_v,
            group_id, sub_group, sub_sub_group,
            weight, handle_a_x, handle_a_y,
            handle_b_x, handle_b_y,
            handle_c_x, handle_c_y,
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
