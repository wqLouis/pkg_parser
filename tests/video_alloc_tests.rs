//! The eager decoding does not copy the frames twice.
//!
//! The planes of a decoded frame are copied out of the buffer of the
//! decoder, which is reused for the next frame: the copy is needed, but
//! the plane must not be copied a second time (into a vector and then
//! into the shared buffer).  The test counts the bytes the allocator
//! hands out while the clip is decoded.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use pkg_parser::pkg_parser::video_parser::{
    FrameDecoder, FramePixels, FrameStatus, VideoError, VideoFrame, VideoInfo,
    decode_all_with_backend, yuv::plane_from_rows,
};

thread_local! {
    /// The number of bytes the allocator handed out to this thread.
    ///
    /// Only the thread that measures is counted: the other threads of
    /// the test harness must not pollute the measurement.
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
    /// Whether the allocations of this thread are counted.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

/// Count the bytes the allocator hands out to this thread.
fn count(size: usize) {
    let _ = ALLOCATED.try_with(|allocated| {
        let _ = COUNTING.try_with(|counting| {
            if counting.get() {
                allocated.set(allocated.get() + size);
            }
        });
    });
}

/// The number of bytes the allocator handed out to this thread so far.
fn allocated() -> usize {
    ALLOCATED.with(Cell::get)
}

/// An allocator that counts the bytes it hands out.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The width of the frames of the clip.
const WIDTH: usize = 32;
/// The height of the frames of the clip.
const HEIGHT: usize = 32;
/// The number of frames of the clip.
const FRAMES: usize = 4;
/// The stride of the planes of the decoder.
///
/// The rows of the chroma planes are padded: the decoder aligns them,
/// so the planes have to be copied row by row.
const STRIDE: usize = 32;

/// A backend that mimics the ffmpeg one.
///
/// The frames are handed out as the planes of a decoded frame: the
/// planes are copied out of a buffer whose rows are padded, exactly like
/// the decoder does it.
struct PaddedBackend {
    /// The buffer the planes are copied out of.
    buffer: Vec<u8>,
    /// The index of the next frame.
    next: usize,
}

impl PaddedBackend {
    /// Build a backend for a clip of four 32x32 frames.
    fn new() -> Self {
        // The luma plane of a frame, then its two chroma planes.
        let buffer = vec![7u8; WIDTH * HEIGHT + 2 * STRIDE * (HEIGHT / 2)];
        Self { buffer, next: 0 }
    }

    /// Copy one plane out of the buffer of the decoder.
    fn plane(&self, offset: usize, width: usize, height: usize) -> Arc<[u8]> {
        plane_from_rows(&self.buffer[offset..], STRIDE, width, height)
            .expect("cannot read the plane of the frame")
    }
}

impl FrameDecoder for PaddedBackend {
    fn name(&self) -> &'static str {
        "padded"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        Ok(VideoInfo {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            duration: 0.4,
            fps: 10.0,
            frame_count: FRAMES as u32,
            codec: "padded".to_string(),
            pixel_format: "yuv420p".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
        if self.next >= FRAMES {
            return Ok(FrameStatus::End);
        }
        self.next += 1;

        let luma = self.plane(0, WIDTH, HEIGHT);
        let blue = self.plane(WIDTH * HEIGHT, WIDTH / 2, HEIGHT / 2);
        let red = self.plane(
            WIDTH * HEIGHT + STRIDE * (HEIGHT / 2),
            WIDTH / 2,
            HEIGHT / 2,
        );

        Ok(FrameStatus::Frame(VideoFrame {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            time: (self.next - 1) as f64 * 0.1,
            duration: 0.1,
            pixels: FramePixels::Yuv420 {
                y: luma,
                u: blue,
                v: red,
                colorspace: pkg_parser::pkg_parser::video_parser::YuvColorspace::Bt601Limited,
            },
        }))
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        self.next = 0;
        Ok(())
    }
}

/// The shared buffer of a plane is allocated once.
///
/// The plane of a frame is copied out of the buffer of the decoder: the
/// shared buffer is allocated with the exact size of the plane and the
/// rows are copied into it, so the plane is never copied twice.
#[test]
fn the_plane_is_copied_into_the_shared_buffer_only_once() {
    // A plane of 16 rows of 16 bytes, with padded rows.
    let data = vec![3u8; 32 * 16];
    COUNTING.with(|counting| counting.set(true));
    let before = allocated();
    let plane = plane_from_rows(&data, 32, 16, 16).expect("cannot read the plane");
    let measured = allocated() - before;
    COUNTING.with(|counting| counting.set(false));

    assert_eq!(plane.len(), 16 * 16);
    assert!(plane.iter().all(|byte| *byte == 3));

    // The shared buffer is allocated once: the size of the plane and the
    // header of the reference count.
    assert!(
        measured <= 16 * 16 + 64,
        "the plane was allocated {measured} bytes for {} bytes of pixels",
        plane.len()
    );
}

/// The eager decoding allocates the frames and nothing else.
///
/// The frames of the clip are kept in memory, so their planes have to be
/// allocated: the decoder must not copy them a second time.
#[test]
fn the_eager_decoding_does_not_copy_the_planes_twice() {
    let backend = Box::new(PaddedBackend::new());

    COUNTING.with(|counting| counting.set(true));
    let before = allocated();
    let clip = decode_all_with_backend(backend, 1024 * 1024).expect("cannot decode the clip");
    let measured = allocated() - before;
    COUNTING.with(|counting| counting.set(false));

    assert_eq!(clip.frames.len(), FRAMES);
    let frames: usize = clip.frames.iter().map(|frame| frame.pixel_bytes()).sum();
    assert_eq!(frames, FRAMES * WIDTH * HEIGHT * 3 / 2);

    // The planes are allocated once: the frames and the vector that
    // holds them are the only thing that is allocated.
    assert!(
        measured <= frames + 2048,
        "the eager decoding allocated {measured} bytes for {frames} bytes of frames"
    );
}
