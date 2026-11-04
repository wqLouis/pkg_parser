//! A micro benchmark of the video decoding path.
//!
//! Usage: cargo run --release --example decode_bench -- <video>
//!
//! It measures the three things that cost CPU when a wallpaper shows a
//! video: the eager decode, the streaming playback and the loop wrap.
//!
//! It measures the two things that were added later as well: the
//! skipping of the frames that are never shown and the decoding that
//! happens ahead of the playback (on the worker thread).

use std::time::{Duration, Instant};

use pkg_parser::pkg_parser::video_parser::{VideoPlayer, decode_all, set_memory_budget};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: decode_bench <video>");
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

    // 5) the same playback, but at half the rate of the clip: the frames
    //    that are never shown do not have to be decoded at all.
    let rate = clip.info.fps / 2.0;
    for (label, skip) in [("every frame", false), ("only the shown ones", true)] {
        let mut player = VideoPlayer::open(&bytes).expect("cannot open the clip");
        if skip {
            player.set_skip_unused_frames(true);
        }
        let start = Instant::now();
        let mut shown = 0usize;
        for step in 0..(clip.frames.len() / 2) {
            let time = step as f64 / rate;
            if player.frame_at(time).ok().flatten().is_some() {
                shown += 1;
            }
        }
        let elapsed = start.elapsed();
        println!(
            "half rate ({label:>19}): {shown} frames in {elapsed:?} ({:?}/frame)",
            elapsed / shown.max(1) as u32
        );
    }

    // 6) the playback of a render loop: the frames are asked for at the
    //    rate of the compositor.  What matters is the time the render
    //    thread spends waiting for the frames.
    //
    //    The decoder that runs on the calling thread blocks the render
    //    loop for the time it takes to decode a frame.  The worker
    //    decodes the frames ahead of the playback: the render thread
    //    does not wait for the decoding at all.
    let mut paced = VideoPlayer::open(&bytes).expect("cannot open the clip");
    paced.set_target_frame_rate(30.0);
    let mut waited = Duration::ZERO;
    for index in 0..clip.frames.len() {
        let time = index as f64 / 30.0;
        let before = Instant::now();
        let _ = paced.frame_at(time);
        waited += before.elapsed();
        // The render loop does something else in between.
        std::thread::sleep(Duration::from_millis(33));
    }
    println!(
        "paced (   on a worker): {waited:?} of waiting ({:?}/frame)",
        waited / frames
    );

    // The same playback, with the decoder on the calling thread.
    #[cfg(feature = "ffmpeg")]
    {
        use pkg_parser::pkg_parser::video_parser::FfmpegBackend;
        use std::sync::Arc;

        let mut inline = VideoPlayer::open_with_backend(
            &bytes,
            Box::new(FfmpegBackend::new(Arc::from(bytes.clone()))),
        )
        .expect("cannot open the clip");
        inline.set_target_frame_rate(30.0);
        let mut waited = Duration::ZERO;
        for index in 0..clip.frames.len() {
            let time = index as f64 / 30.0;
            let before = Instant::now();
            let _ = inline.frame_at(time);
            waited += before.elapsed();
            std::thread::sleep(Duration::from_millis(33));
        }
        println!(
            "paced (in the thread) : {waited:?} of waiting ({:?}/frame)",
            waited / frames
        );
    }
}
