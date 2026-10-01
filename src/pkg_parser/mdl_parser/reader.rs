//! Bounds-checked little-endian byte readers shared by the MDL sections.
//!
//! Every helper advances the cursor and returns `None` when the read would
//! run past the end of the buffer, so a malformed section bails out instead
//! of panicking or reading garbage.

/// Read one byte and advance `pos`.
#[inline(always)]
pub(super) fn read_u8(bytes: &[u8], pos: &mut usize) -> Option<u8> {
    let b = *bytes.get(*pos)?;
    *pos += 1;
    Some(b)
}

/// Read a little-endian `u16` and advance `pos`.
#[inline(always)]
pub(super) fn read_u16(bytes: &[u8], pos: &mut usize) -> Option<u16> {
    let b: &[u8; 2] = bytes.get(*pos..*pos + 2)?.try_into().ok()?;
    *pos += 2;
    Some(u16::from_le_bytes(*b))
}

/// Read a little-endian `u32` and advance `pos`.
#[inline(always)]
pub(super) fn read_u32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let b: &[u8; 4] = bytes.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(u32::from_le_bytes(*b))
}

/// Read a little-endian `f32` and advance `pos`.
#[inline(always)]
pub(super) fn read_f32(bytes: &[u8], pos: &mut usize) -> Option<f32> {
    let b: &[u8; 4] = bytes.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(f32::from_le_bytes(*b))
}

/// Read `N` consecutive little-endian `f32`s (a matrix, for example).
pub(super) fn read_f32_array<const N: usize>(bytes: &[u8], pos: &mut usize) -> Option<[f32; N]> {
    let mut out = [0.0f32; N];
    for slot in &mut out {
        *slot = read_f32(bytes, pos)?;
    }
    Some(out)
}

/// Read a null-terminated UTF-8 string and advance `pos` past the terminator.
///
/// Invalid UTF-8 is replaced (lossy) instead of failing the parse — bone
/// metadata strings are ASCII/JSON, but names may come from any locale.
pub(super) fn read_cstring(bytes: &[u8], pos: &mut usize) -> Option<String> {
    let remaining = bytes.get(*pos..)?;
    let null_pos = remaining.iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&remaining[..null_pos]).into_owned();
    *pos += null_pos + 1;
    Some(s)
}
