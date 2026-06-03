/// Unit tests for mipmap extraction in the tex parser.
///
/// The parser does not synthesize mipmaps on the CPU — that is the
/// renderer's job (the GPU is far faster at it). These tests verify that:
///
/// - For BCn textures, the levels physically present in the payload are
///   split into `mip_levels` (and any gap to the advertised
///   `mipmap_count` is reported).
/// - For every other format, `build_mip_chain` is effectively a no-op
///   that leaves `mip_levels` empty and reports the full gap, so the
///   renderer knows it has to generate the chain on the GPU.
use pkg_parser::pkg_parser::tex_parser::Tex;

/// `build_mip_chain` is a no-op for RGBA: no mip data is stored in the
/// payload, so the parser leaves `mip_levels` empty and the renderer is
/// expected to generate the chain on the GPU.
#[test]
fn test_rgba_mip_chain_left_to_gpu() {
    let w = 4u32;
    let h = 4u32;
    let payload: Vec<u8> = (0..(w * h * 4) as u8).collect();

    let mut tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: payload.len() as u32,
        dimension: [w, h],
        image_count: 1,
        mipmap_count: 3, // 4x4 → 2x2 → 1x1
        actual_mip_count: 1,
        lz4: false,
        decompressed_size: payload.len() as u32,
        extension: "rgba".into(),
        payload,
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();

    // The parser must not synthesize mips.
    assert!(tex.mip_levels.is_empty(), "RGBA: parser must not generate mips on CPU");
    assert_eq!(tex.payload.len(), 64, "Level 0 payload must be untouched");
    assert_eq!(tex.dimension, [4, 4]);

    // Renderer is expected to react to the gap.
    assert_eq!(tex.actual_mip_count(), 1, "Only level 0 is present");
    assert_eq!(tex.missing_mip_count(), 2, "Two levels still need GPU generation");
    assert!(tex.needs_gpu_mip_generation());
}

/// `mipmap_count = 1` is the no-mips case: `build_mip_chain` should be a
/// clean no-op, regardless of the extension.
#[test]
fn test_no_mipmap_when_count_is_one() {
    let mut tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: 16,
        dimension: [2, 2],
        image_count: 1,
        mipmap_count: 1,
        actual_mip_count: 1,
        lz4: false,
        decompressed_size: 16,
        extension: "rgba".into(),
        payload: vec![0u8; 16],
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();
    assert!(tex.mip_levels.is_empty());
    assert_eq!(tex.payload.len(), 16);
    assert_eq!(tex.actual_mip_count(), 1);
    assert_eq!(tex.missing_mip_count(), 0);
    assert!(!tex.needs_gpu_mip_generation());
}

/// BC1 (dxt1) mip levels are concatenated in the payload
/// (`level0 | level1 | ... | level(N-1)`) and the parser splits them into
/// per-level vectors. With a complete 3-level chain on disk, all three
/// levels are present after `build_mip_chain`.
///
/// BC1: 8 bytes per 4×4 block. An 8×8 texture = 2×2 blocks = 32 bytes per level.
#[test]
fn test_bc1_mip_splitting_full_chain() {
    let w = 8u32;
    let h = 8u32;
    let level0_size = (w.div_ceil(4) * h.div_ceil(4) * 8) as usize; // 32 bytes
    let level1_size = (4u32.div_ceil(4) * 4u32.div_ceil(4) * 8) as usize; // 8 bytes
    let level2_size = (2u32.div_ceil(4) * 2u32.div_ceil(4) * 8) as usize; // 8 bytes (1 block)

    let mut payload = vec![0u8; level0_size + level1_size + level2_size];
    // Fill with distinct patterns per level so we can verify the split.
    payload[0..level0_size].fill(0x10);
    payload[level0_size..level0_size + level1_size].fill(0x20);
    payload[level0_size + level1_size..].fill(0x30);

    let mut tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: payload.len() as u32,
        dimension: [w, h],
        image_count: 1,
        mipmap_count: 3,
        actual_mip_count: 1,
        lz4: false,
        decompressed_size: payload.len() as u32,
        extension: "dxt1".into(),
        payload,
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();

    assert_eq!(tex.payload.len(), level0_size);
    assert_eq!(tex.payload[0], 0x10, "Level 0 should be 0x10 pattern");
    assert_eq!(tex.mip_levels.len(), 2);
    assert_eq!(tex.mip_levels[0].len(), level1_size);
    assert_eq!(tex.mip_levels[0][0], 0x20, "Level 1 should be 0x20 pattern");
    assert_eq!(tex.mip_levels[1].len(), level2_size);
    assert_eq!(tex.mip_levels[1][0], 0x30, "Level 2 should be 0x30 pattern");

    // Full chain present.
    assert_eq!(tex.actual_mip_count(), 3);
    assert_eq!(tex.missing_mip_count(), 0);
    assert!(!tex.needs_gpu_mip_generation());
}

/// BC1 payload that's shorter than `mipmap_count` is split up to its last
/// complete level. The remaining gap is reported via `missing_mip_count`.
#[test]
fn test_bc1_mip_splitting_truncated_payload() {
    // Advertise 5 levels for an 8x8 BC1 texture, but only ship 3.
    let w = 8u32;
    let h = 8u32;
    let level0_size = (w.div_ceil(4) * h.div_ceil(4) * 8) as usize; // 32 bytes
    let level1_size = (4u32.div_ceil(4) * 4u32.div_ceil(4) * 8) as usize; // 8 bytes
    let level2_size = (2u32.div_ceil(4) * 2u32.div_ceil(4) * 8) as usize; // 8 bytes (1 block)
    let provided = level0_size + level1_size + level2_size;

    let mut payload = vec![0u8; provided];
    payload[0..level0_size].fill(0x10);
    payload[level0_size..level0_size + level1_size].fill(0x20);
    payload[level0_size + level1_size..].fill(0x30);

    let mut tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: payload.len() as u32,
        dimension: [w, h],
        image_count: 1,
        mipmap_count: 5, // we only have data for 3 of these
        actual_mip_count: 1,
        lz4: false,
        decompressed_size: payload.len() as u32,
        extension: "dxt1".into(),
        payload,
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();

    // The 3 levels physically present are split out.
    assert_eq!(tex.payload.len(), level0_size);
    assert_eq!(tex.payload[0], 0x10);
    assert_eq!(tex.mip_levels.len(), 2);
    assert_eq!(tex.mip_levels[0][0], 0x20);
    assert_eq!(tex.mip_levels[1][0], 0x30);

    // actual_mip_count is the *physical* count, not the advertised one.
    assert_eq!(tex.actual_mip_count(), 3);
    assert_eq!(tex.missing_mip_count(), 2, "5 advertised - 3 present = 2 missing");
    assert!(tex.needs_gpu_mip_generation());
}

/// R8 has no mip data in the payload — same as RGBA, the parser leaves
/// `mip_levels` empty and the renderer is expected to generate the chain.
#[test]
fn test_r8_mip_chain_left_to_gpu() {
    let payload: Vec<u8> = (0..16u8).collect();

    let mut tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: payload.len() as u32,
        dimension: [4, 4],
        image_count: 1,
        mipmap_count: 3, // 4x4 → 2x2 → 1x1
        actual_mip_count: 1,
        lz4: false,
        decompressed_size: payload.len() as u32,
        extension: "r8".into(),
        payload,
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();

    // No CPU synthesis, no extraction — the renderer has to do it all.
    assert!(tex.mip_levels.is_empty(), "R8: parser must not generate mips on CPU");
    assert_eq!(tex.actual_mip_count(), 1);
    assert_eq!(tex.missing_mip_count(), 2);
    assert!(tex.needs_gpu_mip_generation());
}

/// `actual_mip_count` defaults to 1 right after `Tex::new` (only level 0
/// in `payload`) and `missing_mip_count` reflects the gap to the header.
#[test]
fn test_default_state_after_construction() {
    // We can't easily call `Tex::new` here without a full header, so build
    // the struct literally the same way `Tex::new` would after a successful
    // parse of a 64x64 RGBA .tex advertising 8 levels.
    let payload = vec![0u8; 64 * 64 * 4];
    let tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: payload.len() as u32,
        dimension: [64, 64],
        image_count: 1,
        mipmap_count: 8,
        actual_mip_count: 1,
        lz4: false,
        decompressed_size: payload.len() as u32,
        extension: "rgba".into(),
        payload,
        mip_levels: Vec::new(),
    };

    assert_eq!(tex.actual_mip_count(), 1);
    assert_eq!(tex.missing_mip_count(), 7);
    assert!(tex.needs_gpu_mip_generation());
}
