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

The `.mdl` file defines a deformable 2D mesh used for puppet warp animation (Live2D-style). It contains control points, triangle topology, a skeleton, and animation keyframes.

```
+------------------+
|  MDLV Section    |  ← Mesh: control points + triangles
+------------------+
|  MDLS Section    |  ← Skeleton: bones + matrices
+------------------+
|  MDLA Section    |  ← Animation: keyframes
+------------------+
```

## 1. MDLV Section Header

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| `0x00` | 8 | `char[8]` | Magic: `MDLV0023` |
| `0x08` | 4 | `u32` | Type/flags: `0x80000900` |
| `0x0C` | 2 | `u16` | Sub-version: `0x0101` |
| `0x0E` | 2 | `u16` | Flags: `0x0000` |
| `0x10` | 4 | `u32` | Unknown (typically `256`) |
| `0x14` | 1 | `u8` | Padding |
| `0x15` | var | `char[]` | Material path (null-terminated) |
| var | — | — | Zero-padding to alignment |
| var | 4 | `u32` | Marker: `0x80000F00` |
| +4 | 1 | `u8` | Type byte: `0x01` |
| +5 | 3 | `u24` | Control point block size (bytes) |

## Control Point Records

An array of **80-byte records** follows the marker. Each record defines a vertex of the puppet deformation mesh.

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| `+0x00` | 2 | `i16` | X position (pixel-space) |
| `+0x02` | 2 | `i16` | Y position (pixel-space) |
| `+0x04` | 2 | `i16` | U texture coordinate |
| `+0x06` | 2 | `i16` | V texture coordinate |
| `+0x08` | 4 | `u32` | **Group ID** — major part (63–68, 191–196) |
| `+0x0C` | 4 | `u32` | Unknown |
| `+0x10` | 4 | `u32` | Unknown |
| `+0x14` | 4 | `u32` | Constant `0x80000000` |
| `+0x18` | 4 | `u32` | Constant `0x8000003F` |
| `+0x1C` | 4 | `f32` | Float (varies) |
| `+0x20` | 4 | `u32` | **Sub-group ID** (49–55, 175–183) |
| `+0x24` | 4 | `u32` | Constant `0x80000000` |
| `+0x28` | 4 | `u32` | Constant `319` (render layer?) |
| `+0x2C` | 12 | `u8[12]` | Zero padding |
| `+0x38` | 4 | `f32` | Float (27 unique values) |
| `+0x3C` | 4 | `u32` | Flag (`0x0000003F` or `0x80000000`) |
| `+0x40` | 4 | `u32` | **Sub-sub-group ID** (0, 55–63) |
| `+0x44` | 4 | `u32` | Unknown |
| `+0x48` | 4 | `u32` | Raw (varies) |
| `+0x4C` | 4 | `f32` | Float (varies) |

The three group IDs form a **3-level hierarchy** for skeletal deformation.

## Triangle Index Data

After the control points comes a section with a **5-byte header** followed by triangle indices. Each triangle is `3 × u16` (6 bytes).

```
[5-byte header] → [a, b, c] × N triangles
```

## 2. MDLS Section (Skeleton)

```
MDLS0004\0  [u32 next_offset]  [u32 bone_count]  [BONEENTRY × bone_count]
```

Each bone entry:

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| `+0x00` | 1 | `u8` | tmp |
| `+0x01` | 4 | `u32` | Bone type (typically `1`) |
| `+0x05` | 4 | `u32` | Unknown |
| `+0x09` | 4 | `u32` | Entry byte length (typically `64`) |
| `+0x0D` | 64 | `f32[16]` | 4×4 bone matrix |
| `+0x4D` | var | `char[]` | JSON metadata (null-terminated) |

Bone metadata example:
```json
{"a":null,"tp":"36.16162 726.83881 0.00000","tm":100.0}
```

The `tp` field defines the **bone pin position** `(x, y, z)` on the texture.

## 3. MDLA Section (Animation)

```
MDLA0006\0  [u32 end_offset]  [u32 anim_count]  [u32 frame_count]  [u32 pad]
[str name\0]  [str loop\0]  [keyframe_data]
```

| Field | Description |
|-------|-------------|
| `end_offset` | Absolute offset to animation data end |
| `anim_count` | Typically `1` |
| `frame_count` | Number of keyframes (215–812 observed) |
| `name` | Animation name |
| `loop` | Loop mode (e.g. `"loop"`) |
| `keyframe_data` | Raw animation data (format TBD) |
