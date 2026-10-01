# Wallpaper Engine File Format Documentation

This document describes the binary formats used by Wallpaper Engine scene packages.

- **`.pkg`** — Archive container for scene assets
- **`.tex`** — Texture/image files
- **`.mdl`** — Puppet warp model files (2D deformation meshes)

All formats use **Little Endian** byte order and **UTF-8** string encoding unless otherwise noted.

---

# `.pkg` Archive Format

The `.pkg` format is a simple index-based archive. It consists of a **Header**, a **File Table**, and a **Data Blob**.

```
+------------------+
|  HEADER          |
+------------------+
|  FILE TABLE      |
|  (Entry 1..N)    |
+------------------+
|  DATA BLOB       |
|  [File Data]     |
+------------------+
```

## Header

| Offset | Type | Size | Description |
|--------|------|------|-------------|
| `0x00` | `u32` | 4 | Length of version string |
| `0x04` | `char[]` | var | Version string (e.g. `PKGV0022`) |
| after | `u32` | 4 | Number of files in the package |

## File Table Entry (repeated per file)

| Field | Type | Size | Description |
|-------|------|------|-------------|
| Path Length | `u32` | 4 | Byte length of the path string |
| Path | `char[]` | var | Relative file path (e.g. `scene.json`) |
| Offset | `u32` | 4 | Byte offset of file data from data start |
| Size | `u32` | 4 | Byte size of file data |

## Data Blob

Raw file contents concatenated sequentially. Each file's data is located by seeking to `data_start + entry.offset` and reading `entry.size` bytes.

---

# `.tex` Texture Format

## Header

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| `0x00` | 8 | `char[8]` | Version magic (e.g. `TEXV0005`) |
| `0x08` | 1 | — | Separator |
| `0x09` | 8 | `char[8]` | Info magic (e.g. `TEXI0002`) |
| `0x11` | 1 | — | Separator |
| `0x12` | 4 | `u32` | Format ID |
| `0x16` | 4 | — | Padding |
| `0x1A` | 4 | `u32` | Width |
| `0x1E` | 4 | `u32` | Height |
| `0x22` | 12 | — | Padding |
| `0x2E` | 8 | `char[8]` | Block magic (e.g. `TEXB0003`) |
| `0x36` | 1 | — | Separator |
| `0x37` | 4 | `u32` | Image count |
| `0x3B` | 8 | — | Padding |
| `0x43` | 4 | `u32` | Mipmap count |

## Format IDs

| ID | Format | Description |
|----|--------|-------------|
| 0 | raw | Embedded PNG/JPG |
| 4, 7 | dxt1 | BC1/DXT1 compressed |
| 6 | dxt5 | BC3/DXT5 compressed |
| 8 | rg88 | 2-channel uncompressed |
| 9 | r8 | 1-channel uncompressed |

## Payload

### Raw format (ID 0)
| Offset | Type | Size | Description |
|--------|------|------|-------------|
| `0x47` | — | 16 | Padding |
| `0x57` | `u32` | 4 | Payload size |
| `0x5B` | `u8[]` | var | Embedded image data (PNG/JPG) |

### Compressed formats (IDs 4, 6, 7, 8, 9)
| Offset | Type | Size | Description |
|--------|------|------|-------------|
| `0x47` | — | 8 | Padding (mipmap dimensions) |
| `0x4F` | `u32` | 4 | LZ4 flag (1 = compressed) |
| `0x53` | `u32` | 4 | Decompressed size |
| `0x57` | `u32` | 4 | Payload size |
| `0x5B` | `u8[]` | var | Pixel data (decompress with LZ4 if flag set) |

---
# `.mdl` Puppet Model Format

The `.mdl` file defines a deformable 2D mesh used for puppet warp animation
(Live2D-style). An MDL is a **chain of null-terminated sections**; only the
first two are mandatory:

```
+--------------------------------------+
| MDLV0021/MDLV0023  header + mesh     |  control points, triangles,
|                        (+ trailer)   |  index batches
+--------------------------------------+
| MDLS0004  skeleton                   |  bones, bind matrices, metadata
+--------------------------------------+
| MDAT0001  attachments (optional)     |  named sockets parented to bones
+--------------------------------------+
| MDLA0006  animation (optional)       |  clips -> per-bone tracks
+--------------------------------------+
| MDLE0002  bone matrices (optional)   |  one 4x4 matrix per bone
+--------------------------------------+
| 0x00      end-of-file sentinel       |
+--------------------------------------+
```

Each section starts with an 8-byte magic + `\0`. `MDLS`/`MDAT` carry a `u32`
absolute offset to the next section, `MDLA`/`MDLE` carry the absolute offset
where *they* end (which is where the next section starts). Sections may be
missing (models without a skeleton or without animation exist), so the walk
only ever continues at offsets the format itself provides — an unknown magic
ends the walk, it is never searched for.

## 1. MDLV Section (header + mesh)

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| `0x00` | 8 | `char[8]` | Magic: `MDLV0021` / `MDLV0023` |
| `0x08` | 1 | `u8` | Reserved (0) |
| `0x09` | 4 | `u32` | Type word `0x01800009` (same family as the mesh tag `0x0180000F`) |
| `0x0D` | 2 | `u16` | Sub-version (1 observed) |
| `0x0F` | 2 | `u16` | Flags (0 observed) |
| `0x11` | 4 | `u32` | Unknown (0 observed) |
| `0x15` | var | `char[]` | Material path, null-terminated |
| … | 28 | — | Zero padding (constant) |

> The same bytes can be read as `u32@0x08 = 0x80000900`, `u16@0x0C = 0x0101`,
> `u16 = 0`, `u32@0x10 = 256`, `u8` pad — both readings end at offset 21.
> The layout above is kept because the type word then matches the mesh tag.

### Mesh block

```
u32  tag = 0x0180000F   ("0F 00 80 01")
u32  vertex_bytes       (multiple of 80)
     control points     (vertex_bytes / 80 records of 80 bytes)
u32  index_bytes        (multiple of 6)
     triangle indices   (u16, 3 per triangle)
```

The block starts at `21 + len(material_path) + 1 + 28`. That offset is
derived from the header, never searched for: the tag must be there, and if
it isn't the parse fails (`None`) instead of accepting something that merely
looks like a mesh.

### Control point (80 bytes, little-endian)

| Off | Type | Description |
|-----|------|-------------|
| 0 | f32 | `pos_x` (object-local, centred on 0) |
| 4 | f32 | `pos_y` |
| 8 | f32 | `pos_z` (0 for 2D puppets) |
| 12..36 | — | reserved (constants `0.0` / `1.0`) |
| 28 | f32 | per-vertex varied value (unique per vertex) |
| 36 | f32 | `1.0` |
| 40 | u32 ×4 | **bone indices**, one per skinning slot |
| 56 | f32 ×4 | **skinning weights** — always sum to `1.0` |
| 72 | f32 | `tex_u` (0..1) |
| 76 | f32 | `tex_v` (0..1) |

A vertex uses up to 4 bones; unused slots simply carry weight `0.0`.

### Mesh trailer (between the index list and the next section)

```
u8                extra block count (0 or 1 observed)
  per block: u32  type (1), u32 byte length, payload
             payload = 12 bytes/vertex (xyz) when it matches the vertex count
u8                table tag (1)
u32               table bytes (multiple of 16)
  per entry: u32  group id, u32 reserved, u32 start, u32 count
u32               reserved (0)
```

The table splits the index list into draw batches; the `start`/`count` pairs
partition it contiguously. The 12-byte blocks hold the same vertices in a
different origin (a constant offset from the primary positions).

## 2. MDLS Section (skeleton)

```
"MDLS0004" \0
u32   next section offset
u32   bone count
u8    reserved (0)
BONE × bone_count
[undecoded trailing bytes up to `next`]
```

Each bone:

| Size | Type | Description |
|------|------|-------------|
| 4 | `u32` | Bone type (0 = root-ish, 1 = regular observed) |
| 4 | `u32` | Parent index (`0xFFFFFFFF` = no parent) |
| 4 | `u32` | Payload byte length (64 = a 4×4 matrix) |
| 64 | `f32[16]` | Bind-pose 4×4 matrix, **row-major, translation in row 3** |
| var | `char[]` | Info JSON (`tp` = pin position, `tm` = multiplier) |
| var | `char[]` | Bone name (often empty, e.g. `"legs"`) |

Example info string:
```json
{"a":null,"lamax":null,"lamin":null,"rax":null,"ray":null,"raz":null,
 "s":null,"tm":100.0,"tp":"408.81891 0.00000 0.00000"}
```

## 3. MDAT Section (attachments, optional)

```
"MDAT0001" \0
u32   next section offset
u16   attachment count
per attachment:
  u16       bone index the socket is parented to
  char[]    name (e.g. "head", "hair back")
  f32[16]   4×4 row-major transform (translation in row 3)
```

## 4. MDLA Section (animation, optional)

```
"MDLA0006" \0
u32   section end offset (absolute; start of the next section)
u32   animation count
u32   unknown section count (215..3330 observed; NOT the timeline length)
u32   reserved (0)
per clip:
  char[]    clip name ("Animation 1", "eyes", "动画 1", …)
  char[]    loop mode ("loop")
  f32       fps (7.5 .. 60 observed)
  u32       frame count (timeline length)
  u32       reserved (0)
  u32       track count (== bone count)
  per track (clips after the first are separated by zero padding and a small
             unknown prefix):
    u32     reserved (0)
    u32     keyframe bytes (= 36 * (frame_count + 1))
    KEYFRAME × (frame_count + 1)
```

Each keyframe is 36 bytes = 9 little-endian floats:

```
tx, ty, tz,   // translation
rx, ry, rz,   // rotation (radians; rz is the visible 2D rotation)
sx, sy, sz    // scale (always 1,1,1 observed)
```

Keyframes are **dense**: every track stores a sample for every frame, so
playback is a direct index (`frame = floor(t * fps)`), no interpolation of
stored keys is required. Frame 0 reproduces the bone's bind pose exactly —
`rz` matches the rotation of the MDLS/MDLE matrix.

There is one track per bone, in bone order. A file may contain several clips
(e.g. `asuna_body` has `eyes` + `Animation 2`).

## 5. MDLE Section (bone matrices, optional)

```
"MDLE0002" \0
u32   section end offset (absolute)
u32   payload byte length (multiple of 64)
f32[16] × (length / 64)   // one 4×4 matrix per bone, row-major
```

Present only in some files; the matrix count matches the bone count.

## 6. End-of-file sentinel

Every file ends with a single `0x00` byte — an empty section name that
terminates the chain. `MDLA`/`MDLE` end offsets point at it when no further
section follows.

## Parsing

### `MdlFile::new(bytes: &[u8]) -> Option<MdlFile>`

Strict: returns `None` when the bytes are not an MDL (bad magic), when the
mesh block doesn't sit exactly where the header says, or when a section that
*is* present doesn't match the documented layout. Sections that are simply
absent are skipped (empty default), and a truncated buffer never panics.

```rust
pub struct MdlFile {
    pub header: MdlvHeader,       // MDLV header (magic, material path, …)
    pub data: MdlvData,           // control points, triangles, batches
    pub bones: Bones,             // MDLS skeleton (+ undecoded trailing)
    pub attachments: Attachments, // MDAT sockets
    pub animation: Animation,     // MDLA clips/tracks/keyframes
    pub bone_matrices: BoneMatrices, // MDLE matrices
}
```

### `MdlFile::to_json()` / `to_json_compact()`

Serializes the whole model (used by `Pkg::save_pkg` for `*.mdl.json`).
