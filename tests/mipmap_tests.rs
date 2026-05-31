/// Unit tests for mipmap generation in the tex parser.
use pkg_parser::pkg_parser::tex_parser::Tex;

/// Test that a dummy RGBA texture gets correct mip levels.
#[test]
fn test_rgba_mipmap_generation() {
    // Create a 4x4 RGBA texture with known values
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
        lz4: false,
        decompressed_size: payload.len() as u32,
        extension: "rgba".into(),
        payload,
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();

    // Level 0: 4x4 (64 bytes)
    assert_eq!(tex.payload.len(), 64, "Level 0 should be 64 bytes (4x4 RGBA)");
    assert_eq!(tex.dimension, [4, 4]);

    // Level 1: 2x2 (16 bytes)
    assert_eq!(tex.mip_levels.len(), 2, "Should have 2 mip levels (3 total - level 0)");
    assert_eq!(tex.mip_levels[0].len(), 16, "Level 1 should be 16 bytes (2x2 RGBA)");
    
    // Level 2: 1x1 (4 bytes)
    assert_eq!(tex.mip_levels[1].len(), 4, "Level 2 should be 4 bytes (1x1 RGBA)");

    // Verify box-filter averages are correct for a simple pattern
    // level 0: pixel (x,y) = [x*4 + y*16, x*4 + y*16 + 1, x*4 + y*16 + 2, x*4 + y*16 + 3]
    // level 1 pixel (0,0) should be avg of level 0 pixels (0,0),(1,0),(0,1),(1,1)
    let l1 = &tex.mip_levels[0];
    // pixel (0,0) in level 0: bytes[0..4] = [0,1,2,3]; pixel (1,0): bytes[4..8] = [4,5,6,7]; pixel (0,1): bytes[16..20] = [16,17,18,19]; pixel (1,1): bytes[20..24] = [20,21,22,23]
    // Average: r=(0+4+16+20)/4=10, g=(1+5+17+21)/4=11, b=(2+6+18+22)/4=12, a=(3+7+19+23)/4=13
    assert_eq!(l1[0], 10);
    assert_eq!(l1[1], 11);
    assert_eq!(l1[2], 12);
    assert_eq!(l1[3], 13);
}

/// Test that mip_count=1 generates no mip levels.
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
        lz4: false,
        decompressed_size: 16,
        extension: "rgba".into(),
        payload: vec![0u8; 16],
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();
    assert_eq!(tex.mip_levels.len(), 0, "No mips when mipmap_count=1");
    assert_eq!(tex.payload.len(), 16);
}

/// Test BC1 (dxt1) mip level splitting.
/// BC1: 8 bytes per 4x4 block. 8x8 texture = 2x2 blocks = 32 bytes per level.
#[test]
fn test_bc1_mip_splitting() {
    let w = 8u32;
    let h = 8u32;
    let level0_size = (w.div_ceil(4) * h.div_ceil(4) * 8) as usize; // 32 bytes
    let level1_size = (4u32.div_ceil(4) * 4u32.div_ceil(4) * 8) as usize; // 8 bytes
    let level2_size = (2u32.div_ceil(4) * 2u32.div_ceil(4) * 8) as usize; // 8 bytes (1 block)

    let mut payload = vec![0u8; level0_size + level1_size + level2_size];
    // Fill with distinct patterns per level
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
}

/// Test R8 mipmap generation.
#[test]
fn test_r8_mipmap_generation() {
    // 4x4 R8 texture with known pattern
    let mut tex = Tex {
        texv: "TEXV0005".into(),
        texi: "TEXI0001".into(),
        texb: "TEXB0004".into(),
        size: 16,
        dimension: [4, 4],
        image_count: 1,
        mipmap_count: 3,
        lz4: false,
        decompressed_size: 16,
        extension: "r8".into(),
        payload: (0..16u8).collect(),
        mip_levels: Vec::new(),
    };

    tex.build_mip_chain();

    assert_eq!(tex.mip_levels.len(), 2);
    assert_eq!(tex.mip_levels[0].len(), 4, "2x2 = 4 bytes");
    assert_eq!(tex.mip_levels[1].len(), 1, "1x1 = 1 byte");

    // Level 1 pixel averages:
    // [0 1]  [4 5]    avg=2    avg=5
    // [2 3]  [6 7]    avg=3    avg=6  → [2,5,3,6]
    // [8 9]  [12 13]
    // ...
    // Wait, that's the top-left 2x2 block of level 0. Let me recompute.
    // Level 0:
    // 0  1  2  3
    // 4  5  6  7
    // 8  9  10 11
    // 12 13 14 15
    // Level 1: 2x2 = [avg(0,1,4,5), avg(2,3,6,7), avg(8,9,12,13), avg(10,11,14,15)]
    // = [(0+1+4+5)/4, (2+3+6+7)/4, (8+9+12+13)/4, (10+11+14+15)/4]
    // = [2, 4, 10, 12]
    assert_eq!(tex.mip_levels[0][0], 2);
    assert_eq!(tex.mip_levels[0][1], 4);
    assert_eq!(tex.mip_levels[0][2], 10);
    assert_eq!(tex.mip_levels[0][3], 12);
}
