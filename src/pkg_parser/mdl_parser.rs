use std::io::{BufReader, Cursor, Read, Seek};
use serde::Serialize;

/// Read a null-terminated UTF-8 string from a reader.
fn read_cstring<R: Read>(reader: &mut R) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).ok()?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
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
    fn parse<R: Read + Seek>(reader: &mut R, bytes: &[u8]) -> Option<MdlvHeader> {
        let mut magic_buf = [0u8; 8];
        reader.read_exact(&mut magic_buf).ok()?;
        let magic = String::from_utf8_lossy(&magic_buf).into_owned();
        if magic != "MDLV0023" {
            return None;
        }

        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf).ok()?;
        let type_val = u32::from_le_bytes(u32_buf);

        let mut u16_buf = [0u8; 2];
        reader.read_exact(&mut u16_buf).ok()?;
        let sub_version = u16::from_le_bytes(u16_buf);

        reader.read_exact(&mut u16_buf).ok()?;
        let flags = u16::from_le_bytes(u16_buf);

        reader.read_exact(&mut u32_buf).ok()?;
        let unknown_16 = u32::from_le_bytes(u32_buf);

        // Padding byte at offset 20
        let mut pad_byte = [0u8; 1];
        reader.read_exact(&mut pad_byte).ok()?;

        // Material path at offset 21+
        let material_path = read_cstring(reader)?;

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
        reader
            .seek(std::io::SeekFrom::Start(data_start as u64))
            .ok()?;

        let mut marker_buf = [0u8; 4];
        reader.read_exact(&mut marker_buf).ok()?;
        let marker_type = u32::from_le_bytes(marker_buf);
        if marker_type != 0x80000F00 {
            return None;
        }

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

        // Parse remaining data (between records and MDLS) as triangles
        let mdls_pos = bytes.windows(4).position(|w| w == b"MDLS")?;
        let triangles = Self::parse_triangles_from_gap(bytes, record_end, mdls_pos);

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
    fn parse<R: Read + Seek>(reader: &mut R, bytes: &[u8]) -> Option<Bones> {
        let mdls_start = bytes.windows(4).position(|w| w == b"MDLS")?;
        reader.seek(std::io::SeekFrom::Start(mdls_start as u64)).ok()?;

        let header = read_cstring(reader)?;

        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf).ok()?;
        let _next_offset = u32::from_le_bytes(u32_buf);

        reader.read_exact(&mut u32_buf).ok()?;
        let num_bones = u32::from_le_bytes(u32_buf);

        let mut bones = Vec::with_capacity(num_bones as usize);
        for i in 0..num_bones {
            let mut tmp = [0u8; 1];
            reader.read_exact(&mut tmp).ok()?;
            reader.read_exact(&mut u32_buf).ok()?;
            let bone_type = u32::from_le_bytes(u32_buf);
            reader.read_exact(&mut u32_buf).ok()?;
            let unk1 = u32::from_le_bytes(u32_buf);

            reader.read_exact(&mut u32_buf).ok()?;
            let entry_byte_len = u32::from_le_bytes(u32_buf);

            let num_floats = entry_byte_len as usize / 4;
            let mut matrix = [0.0f32; 16];
            for j in 0..num_floats.min(16) {
                let mut f32_buf = [0u8; 4];
                reader.read_exact(&mut f32_buf).ok()?;
                matrix[j] = f32::from_le_bytes(f32_buf);
            }
            if num_floats > 16 {
                reader.seek(std::io::SeekFrom::Current((num_floats - 16) as i64 * 4)).ok()?;
            }

            let info = read_cstring(reader)?;

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
        reader.seek(std::io::SeekFrom::Start(mdla_start as u64)).ok()?;

        let header = read_cstring(reader)?;

        let mut u32_buf = [0u8; 4];
        reader.read_exact(&mut u32_buf).ok()?;
        let end_offset = u32::from_le_bytes(u32_buf);
        reader.read_exact(&mut u32_buf).ok()?;
        let num_animations = u32::from_le_bytes(u32_buf);
        reader.read_exact(&mut u32_buf).ok()?;
        let num_frames = u32::from_le_bytes(u32_buf);
        reader.read_exact(&mut u32_buf).ok()?;
        let _unk = u32::from_le_bytes(u32_buf);

        let animation_name = read_cstring(reader).unwrap_or_default();
        let loop_mode = read_cstring(reader).unwrap_or_default();

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
