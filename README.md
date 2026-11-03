# Wallpaper Engine Pkg Parser

A high-performance Rust library for parsing Wallpaper Engine assets: `.pkg` archives, `.tex` textures, `.mdl` puppet models and video/GIF clips. The command line tool of the `linux-wallpaperengine` project is built on top of it.

## ✨ Features

* **Package Parsing**: Reads a `.pkg` archive into a file map — from a file (`Pkg::new`) or from memory (`Pkg::from_bytes`) — and extracts it with `Pkg::save_pkg`, keeping the original directory structure and converting the textures, the models and the clips on the way.
* **Texture Parsing**: Parses the proprietary `.tex` textures (BC1/BC3, R8, RG88, embedded PNG/JPEG), with their mipmap chain and their real (non-padded) dimensions.
  * *Note: the texture conversion is still **Work In Progress (WIP)** *
* **Puppet Model Parsing**: Parses the `.mdl` puppet warp models — control points, skeleton data, triangle meshes, attachments and animation keyframes — and serialises them to JSON.
* **Video & GIF Parsing**: Detects the container of a clip (mp4, webm, GIF) and decodes it with a pluggable decoder. The frames of a clip can also be written out as PNG files, next to a `<name>.video.json` sidecar.
* **Pluggable Decoders**: Every decoder implements the same `FrameDecoder` trait (`src/pkg_parser/video_parser/backend.rs`): a pure Rust GIF decoder, a pure Rust still image decoder, an ffmpeg decoder (`ffmpeg-next`, behind the optional `ffmpeg` feature) and — in a browser — the decoders of the browser itself. `BackendKind` picks the one to use.
* **Native Pixel Format**: The frames keep the format the decoder produced them in: the ffmpeg backend hands out the YUV 4:2:0 planes of the decoder and the host converts them on the GPU, so no colour conversion costs CPU time. A frame carries its pixels in a `FramePixels` enum (`Rgba` or `Yuv420`).
* **Eager or Streaming Playback**: `decode_all` decodes a whole clip in one go, guarded by a memory budget (256 MiB by default, the `PKG_PARSER_VIDEO_MEMORY_BUDGET` environment variable overrides it): a clip that fits is decoded once and played back by index, a clip that does not fit is streamed by the `VideoPlayer` instead.
* **Browser Support**: The crate builds for `wasm32-unknown-unknown` with the default features — the ffmpeg dependency is only pulled in for the native targets, because ffmpeg cannot be cross compiled. There the decoding is asynchronous: `next_frame` returns a `FrameStatus` (`Frame`, `Pending` or `End`) and `Pending` means "not ready yet, ask again later". The native backends never return it.
* **No Audio**: The crate decodes pictures. The sound of a clip is a playback concern (`decodeAudioData`, `AudioDecoder`, an `<audio>` element) and belongs to the host that renders the wallpaper; the crate only reports whether a container carries an audio stream (`VideoInfo::has_audio`).

## Usage

```rust
use pkg_parser::pkg_parser::video_parser::{VideoPlayer, decode_all};

let bytes = std::fs::read("clip.mp4").expect("cannot read the clip");

// Decode the whole clip at once (it has to fit into the memory budget).
let clip = decode_all(&bytes).expect("cannot decode the clip");
println!("{} frames of {}x{}", clip.frames.len(), clip.info.width, clip.info.height);

// Or stream it: the player decodes the frames while they are played.
let mut player = VideoPlayer::open(&bytes).expect("cannot open the clip");
if let Some(frame) = player.frame_at(1.5).expect("cannot decode the frame") {
    println!("the frame at 1.5s is {}x{}", frame.width, frame.height);
}
```

The binary formats themselves are documented in [src/pkg_parser/README.md](src/pkg_parser/README.md).

## Installation

Add the crate to the `Cargo.toml` of your project:

```toml
[dependencies]
pkg_parser = { git = "https://github.com/wqLouis/depkg.git" }
```

or, when it sits next to the renderer, as a path dependency:

```toml
[dependencies]
pkg_parser = { path = "src/pkg_parser" }
```

The `ffmpeg` feature is on by default and pulls in the ffmpeg decoder. It is only compiled for the native targets, so the browser build works with the default features too:

```bash
cargo check --target wasm32-unknown-unknown
```

Turning the feature off (`default-features = false`) leaves a crate without any native dependency: the GIF decoder, the still image decoder and the browser backends are then the only decoders.

## Acknowledgments & Inspiration

This project was inspired by the incredible work done by the open-source community. Special thanks to:

* **[notscuffed/repkg](https://github.com/notscuffed/repkg)**
* **[AzPepoze/linux-wallpaperengine](https://github.com/AzPepoze/linux-wallpaperengine)**

---

## ⚠️ Disclaimer

**Please respect the copyright of wallpaper creators.**

This tool is intended primarily to assist in bringing Wallpaper Engine content to Linux platforms and for educational/research purposes. We strictly condemn the use of this tool for piracy or unauthorized distribution of paid content.
