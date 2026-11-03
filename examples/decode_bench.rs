//! A micro benchmark of the video decoding path.
//!
//! Usage: cargo run --release --example decode_bench -- <video>
//!
//! It measures the three things that cost CPU when a wallpaper shows a
//! video: the eager decode, the streaming playback and the loop wrap.

use std::time::Instant;

use pkg_parser::pkg_parser::video_parser::{VideoPlayer, decode_all, set_memory_budget};

fn main() {
    let path = std::env::args().nth(1).expect("usage: decode_bench <video>");
    let bytes = std::fs::read(&path).expect("cannot read the video");

    // The eager path is capped by the memory budget; lift it for the
    // benchmark so that a full clip can be decoded.
    set_memory_budget(4 * 1024 * 1024 * 1024);

    // 1) the eager path: the whole clip, decoded once.
    let start = Instant::now();
    let clip = decode_all(&bytes).expect("cannot decode the clip");
    let eager = start.elapsed();
    let frames = clip.frames.len().max(1) as u32;
    println!(
        "eager decode    : {} frames in {eager:?} ({:?}/frame)",
        clip.frames.len(),
        eager / frames
    );

    // 2) the streaming path: one frame_at per displayed frame.
    let mut player = VideoPlayer::open(&bytes).expect("cannot open the clip");
    let start = Instant::now();
    let mut shown = 0usize;
    for index in 0..clip.frames.len() {
        let time = index as f64 / 30.0;
        if player.frame_at(time).ok().flatten().is_some() {
            shown += 1;
        }
    }
    let streaming = start.elapsed();
    println!(
        "streaming       : {shown} frames in {streaming:?} ({:?}/frame)",
        streaming / frames
    );

    // 3) the loop wrap: the player rewinds and decodes again.
    let start = Instant::now();
    let _ = player.frame_at(0.0);
    println!("loop wrap       : {:?}", start.elapsed());

    // 4) the same, but decoding into a smaller texture.
    let mut small = VideoPlayer::open(&bytes).expect("cannot open the clip");
    small.set_output_size(960, 540);
    let start = Instant::now();
    for index in 0..clip.frames.len() {
        let time = index as f64 / 30.0;
        let _ = small.frame_at(time);
    }
    let scaled = start.elapsed();
    println!(
        "streaming @960  : {} frames in {scaled:?} ({:?}/frame)",
        clip.frames.len(),
        scaled / frames
    );
}
