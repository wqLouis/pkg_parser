//! MDLV mesh section: control points, triangles and the mesh trailer
//! (secondary vertex positions + index batches).
//!
//! Nothing is scanned or guessed — the layout is fixed:
//!
//! ```text
//! mesh_offset = header_size (21 + material path + terminator) + 28 padding
//! tag 0x0180000F | u32 vertex_bytes | vertices | u32 index_bytes | indices
//! | trailer (extra vertex blocks + index batch table)
//! ```
//!
//! The trailer ends exactly where the next section starts.  If any part
//! doesn't match the known layout the parse fails with `None` instead of
//! falling back to a byte scan.

use serde::Serialize;

use super::header::MdlvHeader;
use super::reader::{read_f32, read_f32_array, read_u32};

/// Tag starting the mesh block (`0F 00 80 01`).
const MESH_BLOCK_SIGNATURE: [u8; 4] = [0x0F, 0x00, 0x80, 0x01];
const MESH_HEADER_SIZE: usize = 8;
const VERTEX_STRIDE: usize = 80;

/// Parsed mesh: control points, triangles and the trailer tables.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdlvData {
    pub records: Vec<ControlPoint>,
    /// Triangle indices into control points (3 × u16 per triangle).
    pub triangles: Vec<Triangle>,
    /// Draw batches: each covers a contiguous `[start, start+count)` range of
    /// the triangle index list.  Groups are how the editor splits the mesh
    /// into render layers; the ranges partition the whole index buffer.
    pub index_batches: Vec<IndexBatch>,
    /// Optional secondary vertex positions (12 bytes = xyz per vertex).
    ///
    /// Present in some files only.  The values are the same vertices in a
    /// different origin — a constant offset from the primary positions — so
    /// they are redundant for rendering but kept for completeness.
    pub alt_positions: Vec<[f32; 3]>,
}

/// A single 80-byte control point record.
///
/// Layout (decimal offsets, all little-endian):
///
/// | Off | Type | Field |
/// |-----|------|-------|
/// | 0 | f32 | `pos_x` |
/// | 4 | f32 | `pos_y` |
/// | 8 | f32 | `pos_z` (0 for 2D puppets) |
/// | 12..36 | — | reserved (constants `0.0` / `1.0`) |
/// | 28 | f32 | per-vertex varied value (unique per vertex) |
/// | 36 | f32 | `1.0` |
/// | 40 | u32 ×4 | bone indices (one per skinning slot) |
/// | 56 | f32 ×4 | skinning weights (always sum to 1.0) |
/// | 72 | f32 | `tex_u` |
/// | 76 | f32 | `tex_v` |
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPoint {
    pub index: u32,
    /// Position X (object-local, centred around 0).
    pub pos_x: f32,
    /// Position Y (object-local, centred around 0).
    pub pos_y: f32,
    /// Position Z (0 for 2D puppets).
    pub pos_z: f32,
    /// Bone indices of the four skinning slots (an unused slot simply has
    /// weight 0).
    pub bones: [u32; 4],
    /// Skinning weights for the four slots; always sum to `1.0`.
    pub weights: [f32; 4],
    /// Texture U coordinate (range [0, 1]).
    pub tex_u: f32,
    /// Texture V coordinate (range [0, 1]).
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

/// Contiguous slice of the triangle index list (see [`MdlvData::index_batches`]).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexBatch {
    /// Layer/group id (0-based, sequential across the file).
    pub group: u32,
    /// First index (in index units, not triangles) of this batch.
    pub start: u32,
    /// Number of indices in this batch (always a multiple of 3).
    pub count: u32,
}

impl MdlvData {
    /// Parse the mesh block at the offset implied by the header.
    ///
    /// Returns the mesh data and the offset of the next section (right
    /// behind the trailer).
    pub(super) fn parse(bytes: &[u8], header: &MdlvHeader) -> Option<(MdlvData, usize)> {
        let block_offset = header.mesh_offset();
        let (vertex_bytes, index_bytes) = read_mesh_block(bytes, block_offset)?;

        let vertices_offset = block_offset + MESH_HEADER_SIZE;
        let indices_offset = vertices_offset + vertex_bytes as usize + 4; // +4 for indexBytes

        let num_records = vertex_bytes as usize / VERTEX_STRIDE;
        let num_indices = index_bytes as usize / 2; // u16

        // ── Control points ────────────────────────────────────────────────
        let mut records = Vec::with_capacity(num_records);
        for i in 0..num_records {
            let mut p = vertices_offset + i * VERTEX_STRIDE;
            let [pos_x, pos_y, pos_z] = read_f32_array::<3>(bytes, &mut p)?;
            // Bone indices start at +40, weights at +56, UVs at +72.
            p = vertices_offset + i * VERTEX_STRIDE + 40;
            let mut bones = [0u32; 4];
            for slot in &mut bones {
                *slot = read_u32(bytes, &mut p)?;
            }
            p = vertices_offset + i * VERTEX_STRIDE + 56;
            let mut weights = [0.0f32; 4];
            for slot in &mut weights {
                *slot = read_f32(bytes, &mut p)?;
            }
            p = vertices_offset + i * VERTEX_STRIDE + 72;
            let tex_u = read_f32(bytes, &mut p)?;
            let tex_v = read_f32(bytes, &mut p)?;
            records.push(ControlPoint {
                index: i as u32,
                pos_x,
                pos_y,
                pos_z,
                bones,
                weights,
                tex_u,
                tex_v,
            });
        }

        // ── Triangle indices ──────────────────────────────────────────────
        // Every known file stores in-range indices — anything else is
        // garbage, so fail instead of silently dropping triangles.
        let mut triangles = Vec::with_capacity(num_indices / 3);
        let mut off = indices_offset;
        for _ in 0..num_indices / 3 {
            let a = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
            let b = u16::from_le_bytes([bytes[off + 2], bytes[off + 3]]);
            let c = u16::from_le_bytes([bytes[off + 4], bytes[off + 5]]);
            if (a as usize) >= num_records
                || (b as usize) >= num_records
                || (c as usize) >= num_records
            {
                return None;
            }
            triangles.push(Triangle { a, b, c });
            off += 6;
        }

        // ── Trailer: secondary positions + index batches ──────────────────
        let (alt_positions, index_batches, end) = parse_mesh_trailer(bytes, off, num_records)?;

        Some((
            MdlvData {
                records,
                triangles,
                index_batches,
                alt_positions,
            },
            end,
        ))
    }
}

/// The mesh trailer sits between the triangle index list and the next
/// section:
///
/// ```text
/// u8                extra block count
/// extra_block_count × {
///     u32           block type (1 observed)
///     u32           byte length
///     [byte len]    payload (12 bytes per vertex = xyz)
/// }
/// u8                table tag (1 observed)
/// u32               table bytes (multiple of 16)
/// table_bytes / 16 × { u32 group, u32 reserved, u32 start, u32 count }
/// u32               reserved (0 observed)
/// ```
///
/// Returns the tables plus the offset right behind the trailer, which is
/// where the next section starts.  Every field is checked — a mismatch
/// means this is not the known layout, so `None` is returned rather than
/// guessing an end offset.
fn parse_mesh_trailer(
    bytes: &[u8],
    start: usize,
    num_records: usize,
) -> Option<(Vec<[f32; 3]>, Vec<IndexBatch>, usize)> {
    let mut pos = start;
    let mut alt_positions = Vec::new();
    let mut batches = Vec::new();

    // Extra vertex blocks
    let block_count = *bytes.get(pos)?;
    pos += 1;
    for _ in 0..block_count {
        let _block_type = read_u32(bytes, &mut pos)?;
        let len = read_u32(bytes, &mut pos)? as usize;
        let payload = bytes.get(pos..pos + len)?;
        // Only the 12-bytes-per-vertex layout is understood.
        if len == num_records * 12 {
            let mut p = 0;
            for _ in 0..num_records {
                alt_positions.push(read_f32_array::<3>(payload, &mut p)?);
            }
        }
        pos += len;
    }

    // Index batch table
    let tag = *bytes.get(pos)?;
    pos += 1;
    if tag != 1 {
        return None;
    }
    let table_bytes = read_u32(bytes, &mut pos)? as usize;
    if table_bytes % 16 != 0 {
        return None;
    }
    bytes.get(pos..pos + table_bytes + 4)?; // table + trailing reserved u32
    for _ in 0..table_bytes / 16 {
        let group = read_u32(bytes, &mut pos)?;
        let _reserved = read_u32(bytes, &mut pos)?;
        let start = read_u32(bytes, &mut pos)?;
        let count = read_u32(bytes, &mut pos)?;
        batches.push(IndexBatch {
            group,
            start,
            count,
        });
    }
    let _reserved = read_u32(bytes, &mut pos)?;

    Some((alt_positions, batches, pos))
}

/// Validate the mesh block at the exact offset the header implies: tag
/// `0x0180000F`, a vertex byte count that is a multiple of 80 and an index
/// byte count that is a multiple of 6, all inside the buffer.
fn read_mesh_block(bytes: &[u8], block_offset: usize) -> Option<(u32, u32)> {
    let header = bytes.get(block_offset..block_offset + MESH_HEADER_SIZE)?;
    if &header[..4] != &MESH_BLOCK_SIGNATURE {
        return None;
    }
    let vertex_bytes = u32::from_le_bytes(header[4..8].try_into().ok()?);
    if vertex_bytes == 0 || vertex_bytes as usize % VERTEX_STRIDE != 0 {
        return None;
    }

    let vertices_end = block_offset + MESH_HEADER_SIZE + vertex_bytes as usize;
    let index_len = bytes.get(vertices_end..vertices_end + 4)?;
    let index_bytes = u32::from_le_bytes(index_len.try_into().ok()?);
    if index_bytes == 0 || index_bytes as usize % 6 != 0 {
        return None;
    }

    // The index list must fit before the trailer.
    bytes.get(vertices_end + 4 + index_bytes as usize..)?;
    Some((vertex_bytes, index_bytes))
}
