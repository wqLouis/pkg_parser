use std::io::{BufReader, Cursor, Read};

use serde::Serialize;

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

/// MDLV data section (control points, quad topology, triangles).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlvData {
    pub marker_type: u32,
    pub record_block_size: u32,
    pub records: Vec<ControlPoint>,
    pub quads: Vec<Quad>,
    pub triangles: Vec<Triangle>,
}

/// A single 80-byte control point record.
///
/// Each control point defines a vertex of the puppet deformation mesh,
/// with position, texture coordinates, and hierarchical group IDs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPoint {
    pub index: u32,
    /// X position (int16) — pixel-space coordinate on the texture atlas.
    pub pos_x: i16,
    /// Y position (int16) — pixel-space coordinate on the texture atlas.
    pub pos_y: i16,
    /// U texture coordinate (int16).
    pub tex_u: i16,
    /// V texture coordinate (int16).
    pub tex_v: i16,
    /// Major group / bone part ID.
    pub group_id: u32,
    /// Sub-group / sub-part ID.
    pub sub_group: u32,
    /// Sub-sub-group / vertex-type ID.
    pub sub_sub_group: u32,
    /// Raw field at offset +0 (u32).
    pub field_0: u32,
    /// Raw field at offset +4 (u32).
    pub field_4: u32,
    /// Raw field at offset +12 (u32).
    pub field_12: u32,
    /// Raw field at offset +16 (u32).
    pub field_16: u32,
    /// Raw field at offset +28 (f32).
    pub field_28: f32,
    /// Raw field at offset +56 (f32).
    pub field_56: f32,
    /// Raw field at offset +60 (u32).
    pub field_60: u32,
    /// Raw field at offset +72 (u32).
    pub field_72: u32,
    /// Raw field at offset +76 (f32).
    pub field_76: f32,
}

/// A quad defined by 4 vertex indices, forming 2 triangles.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quad {
    pub a: u16,
    pub b: u16,
    pub c: u16,
    pub d: u16,
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
    /// Parse an MDL file from raw bytes.
    pub fn new(bytes: &[u8]) -> Option<MdlFile> {
        let mut cursor = Cursor::new(bytes);
        let mut buf_reader = BufReader::new(&mut cursor);

        let header = MdlvHeader::parse(&mut buf_reader, bytes)?;
        let data = MdlvData::parse(&mut buf_reader, bytes, header.header_size)?;
        let bones = Bones::parse(&mut buf_reader, bytes)?;
        let animation = Animation::parse(&mut buf_reader, bytes)?;

        Some(MdlFile {
            header,
            data,
            bones,
            animation,
        })
    }

    /// Serialize the parsed MDL file to a pretty-printed JSON string.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Serialize the parsed MDL file to a compact JSON string.
    pub fn to_json_compact(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

impl MdlvHeader {
    fn parse<R: Read + Seek>(reader: &mut R, bytes: &[u8]) -> Option<MdlvHeader> {
        // Magic: "MDLV0023" (8 bytes)
        let mut magic_buf = [0u8; 8];
        reader.read_exact(&mut magic_buf).ok()?;
        let magic = String::from_utf8_lossy(&magic_buf).into_owned();
        if magic != "MDLV0023" {
            return None;
        }

        // Offset 8: type/flags (u32)
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf).ok()?;
        let type_val = u32::from_le_bytes(u32_buf);

        // Offset 12: version (u16)
        let mut u16_buf = [0u8; 2];
        reader.read_exact(&mut u16_buf).ok()?;
        let sub_version = u16::from_le_bytes(u16_buf);

        // Offset 14: flags (u16)
        reader.read_exact(&mut u16_buf).ok()?;
        let flags = u16::from_le_bytes(u16_buf);

        // Offset 16: unknown constant (u32)
        reader.read_exact(&mut u32_buf).ok()?;
        let unknown_16 = u32::from_le_bytes(u32_buf);

        // Offset 20: single null padding byte
        let mut pad_byte = [0u8; 1];
        reader.read_exact(&mut pad_byte).ok()?;

        // Offset 21+: null-terminated material path string
        let mut path_bytes = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).ok()?;
            if byte[0] == 0 {
                break;
            }
            path_bytes.push(byte[0]);
        }
        let material_path = String::from_utf8_lossy(&path_bytes).into_owned();

        // Header size: find the 00 0f 00 80 marker
        let header_size = bytes
            .windows(4)
            .position(|w| w == [0x00, 0x0f, 0x00, 0x80])
            .unwrap_or(bytes.len());

        Some(MdlvHeader {
            magic,
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
    fn parse<R: Read + Seek>(reader: &mut R, bytes: &[u8], data_start: usize) -> Option<MdlvData> {
        // Seek to the data section start
        reader
            .seek(std::io::SeekFrom::Start(data_start as u64))
            .ok()?;

        // Data section marker: 00 0f 00 80 (4 bytes)
        let mut marker_buf = [0u8; 4];
        reader.read_exact(&mut marker_buf).ok()?;
        let marker_type = u32::from_le_bytes(marker_buf);
        if marker_type != 0x80000F00 {
            return None;
        }

        // Next: 1 byte type byte + 3-byte u24 LE size
        let mut type_byte = [0u8; 1];
        reader.read_exact(&mut type_byte).ok()?;

        let mut size_buf = [0u8; 3];
        reader.read_exact(&mut size_buf).ok()?;
        let record_block_size =
            u32::from_le_bytes([size_buf[0], size_buf[1], size_buf[2], 0]);

        // Parse control point records
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

        // Remaining section: contains quad topology
        let idx_start = Self::find_idx_start(bytes, record_end)?;
        let remaining_start = record_end;
        let remaining_size = idx_start - remaining_start;

        let quads = Self::parse_quads(bytes, remaining_start, remaining_size);

        // Triangle indices (always 10002 bytes)
        let triangles = Self::parse_triangles(bytes, idx_start);

        Some(MdlvData {
            marker_type,
            record_block_size,
            records,
            quads,
            triangles,
        })
    }

    /// Find where the triangle index data starts (always 10002 bytes before MDLS).
    fn find_idx_start(bytes: &[u8], _after_records: usize) -> Option<usize> {
        let mdls_pos = bytes.windows(4).position(|w| w == b"MDLS")?;
        Some(mdls_pos - 10002)
    }

    fn parse_quads(bytes: &[u8], start: usize, size: usize) -> Vec<Quad> {
        let mut quads = Vec::new();

        // First 5 bytes: header (skip)
        let data_start = start + 5;
        let data_size = size.saturating_sub(5);
        let num_u16 = data_size / 2;

        for i in (0..num_u16).step_by(6) {
            if data_start + (i + 5) * 2 > start + size {
                break;
            }
            let a = u16::from_le_bytes([
                bytes[data_start + i * 2],
                bytes[data_start + i * 2 + 1],
            ]);
            let b = u16::from_le_bytes([
                bytes[data_start + (i + 1) * 2],
                bytes[data_start + (i + 1) * 2 + 1],
            ]);
            let c = u16::from_le_bytes([
                bytes[data_start + (i + 2) * 2],
                bytes[data_start + (i + 2) * 2 + 1],
            ]);
            let d = u16::from_le_bytes([
                bytes[data_start + (i + 5) * 2],
                bytes[data_start + (i + 5) * 2 + 1],
            ]);
            quads.push(Quad { a, b, c, d });
        }

        quads
    }

    fn parse_triangles(bytes: &[u8], start: usize) -> Vec<Triangle> {
        let mut triangles = Vec::new();
        // Triangle data is always 10002 bytes = 1667 triangles
        let size = 10002usize;

        for j in (0..size).step_by(6) {
            if start + j + 6 > bytes.len() {
                break;
            }
            let a = u16::from_le_bytes([bytes[start + j], bytes[start + j + 1]]);
            let b = u16::from_le_bytes([bytes[start + j + 2], bytes[start + j + 3]]);
            let c = u16::from_le_bytes([bytes[start + j + 4], bytes[start + j + 5]]);
            triangles.push(Triangle { a, b, c });
        }

        triangles
    }
}

impl ControlPoint {
    fn parse(bytes: &[u8], offset: usize, index: u32) -> ControlPoint {
        let field_0 = u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let field_4 = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);

        let pos_x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let pos_y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let tex_u = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let tex_v = i16::from_le_bytes([bytes[offset + 6], bytes[offset + 7]]);

        let group_id = u32::from_le_bytes([
            bytes[offset + 8],
            bytes[offset + 9],
            bytes[offset + 10],
            bytes[offset + 11],
        ]);
        let field_12 = u32::from_le_bytes([
            bytes[offset + 12],
            bytes[offset + 13],
            bytes[offset + 14],
            bytes[offset + 15],
        ]);
        let field_16 = u32::from_le_bytes([
            bytes[offset + 16],
            bytes[offset + 17],
            bytes[offset + 18],
            bytes[offset + 19],
        ]);

        let field_28 = f32::from_le_bytes([
            bytes[offset + 28],
            bytes[offset + 29],
            bytes[offset + 30],
            bytes[offset + 31],
        ]);

        let sub_group = u32::from_le_bytes([
            bytes[offset + 32],
            bytes[offset + 33],
            bytes[offset + 34],
            bytes[offset + 35],
        ]);

        let sub_sub_group = u32::from_le_bytes([
            bytes[offset + 64],
            bytes[offset + 65],
            bytes[offset + 66],
            bytes[offset + 67],
        ]);

        let field_56 = f32::from_le_bytes([
            bytes[offset + 56],
            bytes[offset + 57],
            bytes[offset + 58],
            bytes[offset + 59],
        ]);
        let field_60 = u32::from_le_bytes([
            bytes[offset + 60],
            bytes[offset + 61],
            bytes[offset + 62],
            bytes[offset + 63],
        ]);
        let field_72 = u32::from_le_bytes([
            bytes[offset + 72],
            bytes[offset + 73],
            bytes[offset + 74],
            bytes[offset + 75],
        ]);
        let field_76 = f32::from_le_bytes([
            bytes[offset + 76],
            bytes[offset + 77],
            bytes[offset + 78],
            bytes[offset + 79],
        ]);

        ControlPoint {
            index,
            pos_x,
            pos_y,
            tex_u,
            tex_v,
            group_id,
            sub_group,
            sub_sub_group,
            field_0,
            field_4,
            field_12,
            field_16,
            field_28,
            field_56,
            field_60,
            field_72,
            field_76,
        }
    }
}

impl Bones {
    fn parse<R: Read + Seek>(reader: &mut R, bytes: &[u8]) -> Option<Bones> {
        // Find the MDLS section
        let mdls_start = bytes.windows(4).position(|w| w == b"MDLS")?;

        reader.seek(std::io::SeekFrom::Start(mdls_start as u64)).ok()?;

        // Read "MDLS" header (null-terminated)
        let mut header_bytes = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).ok()?;
            if byte[0] == 0 {
                break;
            }
            header_bytes.push(byte[0]);
        }
        let header = String::from_utf8_lossy(&header_bytes).into_owned();

        // u32: next section offset (MDLA)
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf).ok()?;
        let _next_offset = u32::from_le_bytes(u32_buf);

        // u32: number of bones
        reader.read_exact(&mut u32_buf).ok()?;
        let num_bones = u32::from_le_bytes(u32_buf);

        let mut bones = Vec::with_capacity(num_bones as usize);
        for i in 0..num_bones {
            // BONEENTRYHEADER: BYTE tmp, DWORD type, DWORD unk1 (9 bytes)
            let mut tmp = [0u8; 1];
            reader.read_exact(&mut tmp).ok()?;
            reader.read_exact(&mut u32_buf).ok()?;
            let bone_type = u32::from_le_bytes(u32_buf);
            reader.read_exact(&mut u32_buf).ok()?;
            let unk1 = u32::from_le_bytes(u32_buf);

            // DWORD entryByteLength
            reader.read_exact(&mut u32_buf).ok()?;
            let entry_byte_len = u32::from_le_bytes(u32_buf);

            // float matrix[entryByteLength / 4]
            let num_floats = entry_byte_len as usize / 4;
            let mut matrix = [0.0f32; 16];
            for j in 0..num_floats.min(16) {
                let mut f32_buf = [0u8; 4];
                reader.read_exact(&mut f32_buf).ok()?;
                matrix[j] = f32::from_le_bytes(f32_buf);
            }
            // Skip any remaining floats beyond 16
            if num_floats > 16 {
                reader
                    .seek(std::io::SeekFrom::Current((num_floats - 16) as i64 * 4))
                    .ok()?;
            }

            // CHAR info[] (null-terminated string)
            let mut info_bytes = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                reader.read_exact(&mut byte).ok()?;
                if byte[0] == 0 {
                    break;
                }
                info_bytes.push(byte[0]);
            }
            let info = String::from_utf8_lossy(&info_bytes).into_owned();

            bones.push(BoneEntry {
                index: i,
                tmp: tmp[0],
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
    fn parse<R: Read + Seek>(reader: &mut R, bytes: &[u8]) -> Option<Animation> {
        let mdla_start = bytes.windows(4).position(|w| w == b"MDLA")?;

        reader
            .seek(std::io::SeekFrom::Start(mdla_start as u64))
            .ok()?;

        // Read "MDLA" header (null-terminated)
        let mut header_bytes = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).ok()?;
            if byte[0] == 0 {
                break;
            }
            header_bytes.push(byte[0]);
        }
        let header = String::from_utf8_lossy(&header_bytes).into_owned();

        // 4 DWORDs
        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf).ok()?;
        let end_offset = u32::from_le_bytes(u32_buf);
        reader.read_exact(&mut u32_buf).ok()?;
        let num_animations = u32::from_le_bytes(u32_buf);
        reader.read_exact(&mut u32_buf).ok()?;
        let num_frames = u32::from_le_bytes(u32_buf);
        reader.read_exact(&mut u32_buf).ok()?;
        let _unk = u32::from_le_bytes(u32_buf);

        // Read null-terminated strings
        let mut strings = Vec::new();
        for _ in 0..10 {
            let mut s_bytes = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                if reader.read_exact(&mut byte).is_err() {
                    break;
                }
                if byte[0] == 0 {
                    if !s_bytes.is_empty() {
                        strings.push(String::from_utf8_lossy(&s_bytes).into_owned());
                    }
                    break;
                }
                s_bytes.push(byte[0]);
            }
            if strings.len() >= 2 {
                break;
            }
        }

        let animation_name = strings.first().cloned().unwrap_or_default();
        let loop_mode = strings.get(1).cloned().unwrap_or_default();

        // Read the rest as animation data
        let data_start = reader.stream_position().ok()? as usize;
        let animation_data = bytes[data_start..].to_vec();

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

// Helper trait to allow seeking on BufReader<Cursor<&[u8]>>
use std::io::Seek;
