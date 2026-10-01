//! Integration tests for the MDL (puppet model) parser.
//!
//! Every `.mdl` shipped in the repository's `test/` directory is parsed —
//! both loose files and the ones packed inside the `.pkg` fixtures — and the
//! structural invariants discovered while documenting the format are
//! asserted:
//!
//! - mesh: non-empty, all triangle indices in range, skinning weights sum
//!   to 1.0, index batches partition the index buffer
//! - skeleton: parent indices resolve, MDLE (when present) holds exactly
//!   one matrix per bone
//! - animation: one track per bone, `frame_count + 1` keyframes per track
//! - the whole parse is fail-safe: truncated garbage never panics

use std::path::{Path, PathBuf};

use pkg_parser::pkg_parser::{mdl_parser::MdlFile, parser::Pkg};

/// Repository `test/` directory (the crate lives in `src/pkg_parser`).
fn test_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("test")
}

/// Collect every `.mdl` blob available: loose files plus all `.mdl` entries
/// of the `.pkg` fixtures.
fn collect_models() -> Vec<(String, Vec<u8>)> {
    let mut models = Vec::new();
    let root = test_dir();

    collect_loose_files(&root, &mut models);

    let Ok(entries) = std::fs::read_dir(&root) else {
        return models;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pkg") {
            continue;
        }
        let Ok(pkg) = Pkg::new(&path) else {
            continue;
        };
        for (name, bytes) in &pkg.files {
            if name.to_lowercase().ends_with(".mdl") {
                models.push((format!("{}:{}", path.display(), name), bytes.clone()));
            }
        }
    }
    models
}

fn collect_loose_files(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_loose_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("mdl") {
            if let Ok(bytes) = std::fs::read(&path) {
                out.push((path.display().to_string(), bytes));
            }
        }
    }
}

#[test]
fn test_every_sample_model_parses() {
    let models = collect_models();
    assert!(
        models.len() >= 20,
        "expected the fixture set, found only {} models",
        models.len()
    );

    for (name, bytes) in &models {
        let mdl = MdlFile::new(bytes)
            .unwrap_or_else(|| panic!("{name}: failed to parse a valid MDL"));

        assert!(
            mdl.header.magic.starts_with("MDLV"),
            "{name}: unexpected magic {}",
            mdl.header.magic
        );
        assert!(
            !mdl.header.material_path.is_empty(),
            "{name}: material path missing"
        );
    }
}

#[test]
fn test_mesh_invariants() {
    for (name, bytes) in &collect_models() {
        let mdl = MdlFile::new(bytes).unwrap_or_else(|| panic!("{name}"));

        assert!(!mdl.data.records.is_empty(), "{name}: no control points");
        assert!(!mdl.data.triangles.is_empty(), "{name}: no triangles");

        let num_records = mdl.data.records.len();
        for t in &mdl.data.triangles {
            assert!(
                (t.a as usize) < num_records
                    && (t.b as usize) < num_records
                    && (t.c as usize) < num_records,
                "{name}: triangle index out of range"
            );
        }

        // Skinning: the four weights always sum to 1.0 and every bone
        // reference resolves into the skeleton.
        for cp in &mdl.data.records {
            let sum: f32 = cp.weights.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-3,
                "{name}: weights of record {} sum to {sum}",
                cp.index
            );
            for (slot, bone) in cp.bones.iter().enumerate() {
                if cp.weights[slot] != 0.0 {
                    assert!(
                        (*bone as usize) < mdl.bones.bones.len(),
                        "{name}: record {} references bone {bone} of {}",
                        cp.index,
                        mdl.bones.bones.len()
                    );
                }
            }
        }

        // Index batches (when present) partition the index buffer.
        if !mdl.data.index_batches.is_empty() {
            let mut cursor = 0u32;
            for batch in &mdl.data.index_batches {
                assert_eq!(
                    batch.start, cursor,
                    "{name}: batch {} does not start where the previous ended",
                    batch.group
                );
                assert_eq!(batch.count % 3, 0, "{name}: batch size not triangular");
                cursor += batch.count;
            }
            assert_eq!(
                cursor as usize,
                mdl.data.triangles.len() * 3,
                "{name}: batches cover {cursor} indices, mesh has {}",
                mdl.data.triangles.len() * 3
            );
        }
    }
}

#[test]
fn test_skeleton_invariants() {
    for (name, bytes) in &collect_models() {
        let mdl = MdlFile::new(bytes).unwrap_or_else(|| panic!("{name}"));
        let num_bones = mdl.bones.bones.len();
        assert!(num_bones > 0, "{name}: no bones");

        for bone in &mdl.bones.bones {
            // 0xFFFFFFFF marks a root; anything else must resolve.
            if bone.parent_index != u32::MAX {
                assert!(
                    (bone.parent_index as usize) < num_bones,
                    "{name}: bone {} has bogus parent {}",
                    bone.index,
                    bone.parent_index
                );
            }
        }

        // MDLE, when present, stores exactly one matrix per bone.
        if !mdl.bone_matrices.matrices.is_empty() {
            assert_eq!(
                mdl.bone_matrices.matrices.len(),
                num_bones,
                "{name}: MDLE matrix count differs from bone count"
            );
        }
    }
}

#[test]
fn test_animation_invariants() {
    let mut saw_animation = 0;
    let mut saw_multi_clip = 0;

    for (name, bytes) in &collect_models() {
        let mdl = MdlFile::new(bytes).unwrap_or_else(|| panic!("{name}"));

        if mdl.animation.clips.is_empty() {
            // Animation is optional — a few models ship without one.
            assert_eq!(mdl.animation.num_animations, 0, "{name}");
            continue;
        }
        saw_animation += 1;
        assert_eq!(
            mdl.animation.clips.len() as u32,
            mdl.animation.num_animations,
            "{name}: clip count differs from the section header"
        );
        if mdl.animation.clips.len() > 1 {
            saw_multi_clip += 1;
        }

        assert_eq!(
            mdl.animation.animation_name, mdl.animation.clips[0].name,
            "{name}: convenience fields must mirror the first clip"
        );

        for clip in &mdl.animation.clips {
            assert!(!clip.name.is_empty(), "{name}: unnamed clip");
            assert!(clip.fps > 0.0, "{name}: clip {} has no fps", clip.name);
            assert_eq!(
                clip.tracks.len(),
                mdl.bones.bones.len(),
                "{name}: clip {} has {} tracks for {} bones",
                clip.name,
                clip.tracks.len(),
                mdl.bones.bones.len()
            );
            for track in &clip.tracks {
                assert_eq!(
                    track.keyframes.len() as u32,
                    clip.frame_count + 1,
                    "{name}: clip {} track length",
                    clip.name
                );
            }
        }
    }

    assert!(saw_animation > 0, "no fixture model carried an animation");
    assert!(saw_multi_clip > 0, "no fixture model carried multiple clips");
}

#[test]
fn test_mesh_block_is_never_guessed() {
    // The mesh block must sit exactly where the header says. Corrupting its
    // tag must fail the parse — there is no byte-scan fallback that could
    // latch onto something that merely looks like a mesh.
    let (name, bytes) = collect_models().into_iter().next().expect("no fixture model");
    let tag = [0x0F, 0x00, 0x80, 0x01];
    let mut corrupted = bytes.clone();
    let offset = corrupted
        .windows(4)
        .position(|w| w == tag)
        .unwrap_or_else(|| panic!("{name}: mesh tag not found"));
    corrupted[offset] ^= 0xFF;

    assert!(
        MdlFile::new(&corrupted).is_none(),
        "{name}: parser must reject a file whose mesh tag is wrong"
    );
}

#[test]
fn test_missing_skeleton_is_tolerated() {
    // Not every file has to carry a skeleton — breaking the MDLS magic must
    // not break the mesh parse, it just means "no skeleton found".
    let (name, bytes) = collect_models().into_iter().next().expect("no fixture model");
    let mut corrupted = bytes.clone();
    let offset = corrupted
        .windows(4)
        .position(|w| w == b"MDLS")
        .unwrap_or_else(|| panic!("{name}: MDLS marker not found"));
    corrupted[offset..offset + 4].copy_from_slice(b"XXXX");

    let mdl =
        MdlFile::new(&corrupted).unwrap_or_else(|| panic!("{name}: mesh must still parse"));
    assert!(!mdl.data.records.is_empty(), "{name}: mesh lost");
    assert!(
        mdl.bones.bones.is_empty(),
        "{name}: skeleton must be reported as absent"
    );
}

#[test]
fn test_truncated_input_is_fail_safe() {
    // A valid model truncated to any length must never panic.
    let models = collect_models();
    let (name, bytes) = models.first().expect("no fixture model");

    for len in [0usize, 4, 20, 21, 64, 200, 500, bytes.len() / 2] {
        let slice = &bytes[..len.min(bytes.len())];
        // Result may be Some or None, it just must not panic.
        let _ = MdlFile::new(slice);
    }

    // Random garbage is rejected outright.
    assert!(MdlFile::new(&[0u8; 128]).is_none(), "{name}");
}

#[test]
fn test_json_serialization() {
    let models = collect_models();
    let (name, bytes) = models.first().expect("no fixture model");
    let mdl = MdlFile::new(bytes).expect("fixture model must parse");

    let json = mdl.to_json().expect("pretty JSON");
    assert!(json.contains("materialPath"), "unexpected JSON shape");
    let compact = mdl.to_json_compact().expect("compact JSON");
    assert!(compact.len() <= json.len());
    assert!(compact.contains(name.split('/').next().unwrap_or("")), "{name}");
}
