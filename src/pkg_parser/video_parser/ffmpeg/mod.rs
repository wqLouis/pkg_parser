//! ffmpeg backed video decoding.
//!
//! This module is the *only* place in the crate that knows about
//! `ffmpeg_next`: every error is translated into a
//! [`VideoError`](crate::pkg_parser::video_parser::info::VideoError) so that
//! no foreign type leaks into the public API.  The module is compiled only
//! when the `ffmpeg` cargo feature is enabled, which keeps the crate free of
//! any native dependency in the browser (wasm) build.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::decoder;
use ffmpeg_next::util::frame::video::Video as Frame;
use log::{debug, warn};

use crate::pkg_parser::video_parser::backend::{FrameDecoder, detect_format};
use crate::pkg_parser::video_parser::info::{VideoError, VideoFormat, VideoFrame, VideoInfo};

/// How many packets are pulled from the demuxer at once.
///
/// The packets are decoded one by one, so a small batch keeps the memory
/// footprint low without re-creating the demuxer iterator for every frame.
const PACKET_BATCH: usize = 32;

/// ffmpeg-backed decoder. Owns the container bytes and re-opens the demuxer on `rewind`.
pub struct FfmpegBackend {
    /// The raw container payload.
    bytes: Arc<[u8]>,
    /// File name hint for the demuxer (it picks the demuxer from it).
    hint: &'static str,
    /// The demuxer; `None` until the container was opened.
    ictx: Option<ffmpeg::format::context::Input>,
    /// The video decoder.
    decoder: Option<decoder::Video>,
    /// The swscale context converting the decoded frames to RGBA.
    scaler: Option<ffmpeg::software::scaling::Context>,
    /// Buffer the decoder writes the decoded frames into.
    frame_buf: Option<Frame>,
    /// Buffer the scaler writes the RGBA frames into.
    out_buf: Option<Frame>,
    /// Pixel format of the decoded frames.
    src_format: ffmpeg::format::Pixel,
    /// Size of the decoded frames.
    src_size: (u32, u32),
    /// Size of the frames handed out to the caller.
    out_size: (u32, u32),
    /// Time base of the video stream (numerator / denominator).
    time_base: (i32, i32),
    /// Frames per second of the video stream.
    fps: f64,
    /// Duration of the video stream, in time base units.
    stream_duration: i64,
    /// Duration of the container, in microseconds.
    container_duration: i64,
    /// Index of the video stream inside the container.
    video_index: usize,
    /// Name of the codec of the video stream (e.g. `"h264"`).
    codec_name: String,
    /// Packets that were demuxed but not decoded yet.
    packets: VecDeque<ffmpeg::packet::Packet>,
    /// Whether the end of the stream was already signalled to the decoder.
    eof_sent: bool,
    /// Fallback timestamp for the frames that carry no PTS.
    fallback_time: f64,
}

impl FfmpegBackend {
    /// Create a backend for the given container payload.
    ///
    /// The container itself is opened lazily, when the backend is probed.
    pub fn new(bytes: Arc<[u8]>) -> Self {
        // The demuxer is picked from the file name hint.
        let hint = match detect_format(&bytes) {
            Some(VideoFormat::Mp4) => "video.mp4",
            Some(VideoFormat::WebM) => "video.webm",
            _ => "video.avi",
        };

        Self {
            bytes,
            hint,
            ictx: None,
            decoder: None,
            scaler: None,
            frame_buf: None,
            out_buf: None,
            src_format: ffmpeg::format::Pixel::None,
            src_size: (0, 0),
            out_size: (0, 0),
            time_base: (0, 1),
            fps: 0.0,
            stream_duration: 0,
            container_duration: 0,
            video_index: 0,
            codec_name: String::new(),
            packets: VecDeque::new(),
            eof_sent: false,
            fallback_time: 0.0,
        }
    }

    /// Open the container and the decoder, if that did not happen yet.
    fn ensure_open(&mut self) -> Result<(), VideoError> {
        if self.ictx.is_some() {
            return Ok(());
        }
        self.open_container()
    }

    /// Demux the container and open the video decoder.
    fn open_container(&mut self) -> Result<(), VideoError> {
        init_ffmpeg()?;

        // The payload is decoded straight from memory: no temporary file.
        let io = ffmpeg::format::context::StreamIo::from_read_seek(Cursor::new(self.bytes.clone()))
            .map_err(|error| decode_error(format!("cannot open the container: {error:?}")))?;
        let ictx = ffmpeg::format::input_from_stream(io, Some(self.hint), None)
            .map_err(|error| decode_error(format!("cannot demux `{}`: {error:?}", self.hint)))?;

        let (video_index, time_base, stream_duration, parameters) = {
            let streams = ictx.streams();
            let stream = streams.best(ffmpeg::media::Type::Video).ok_or_else(|| {
                VideoError::Unsupported("the container has no video stream".to_string())
            })?;
            (
                stream.index(),
                stream.time_base(),
                stream.duration(),
                stream.parameters(),
            )
        };
        let codec_name = parameters.id().name().to_string();

        let context = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|error| decode_error(format!("cannot create the decoder: {error:?}")))?;
        // `video()` looks the codec up by its id and opens the decoder.
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| decode_error(format!("cannot open the decoder: {error:?}")))?;

        let src_format = decoder.format();
        let src_size = (decoder.width(), decoder.height());
        let fps = decoder
            .frame_rate()
            .map(|rate| f64::from(rate.numerator()) / f64::from(rate.denominator()))
            .unwrap_or(0.0);
        let container_duration = ictx.duration();

        self.ictx = Some(ictx);
        self.decoder = Some(decoder);
        self.video_index = video_index;
        self.codec_name = codec_name;
        self.time_base = (time_base.numerator(), time_base.denominator());
        self.stream_duration = stream_duration;
        self.container_duration = container_duration;
        self.src_format = src_format;
        self.src_size = src_size;
        self.fps = fps;
        self.packets.clear();
        self.eof_sent = false;
        self.fallback_time = 0.0;

        // Without an explicit request the frames are handed out in their
        // native resolution.
        if self.out_size == (0, 0) {
            self.out_size = src_size;
        }
        self.build_buffers()?;

        debug!(
            "opened `{}`: {}x{} @ {} fps, {} codec",
            self.hint, src_size.0, src_size.1, fps, self.codec_name
        );
        Ok(())
    }

    /// (Re)create the frame buffers and the swscale context.
    fn build_buffers(&mut self) -> Result<(), VideoError> {
        let (src_width, src_height) = self.src_size;
        let (out_width, out_height) = self.out_size;
        if src_width == 0 || src_height == 0 || out_width == 0 || out_height == 0 {
            return Err(decode_error("the video has no dimensions"));
        }

        self.frame_buf = Some(Frame::new(self.src_format, src_width, src_height));
        self.out_buf = Some(Frame::new(
            ffmpeg::format::Pixel::RGBA,
            out_width,
            out_height,
        ));
        self.scaler = Some(
            ffmpeg::software::scaling::Context::get(
                self.src_format,
                src_width,
                src_height,
                ffmpeg::format::Pixel::RGBA,
                out_width,
                out_height,
                ffmpeg::software::scaling::Flags::BILINEAR,
            )
            .map_err(|error| decode_error(format!("cannot create the scaler: {error:?}")))?,
        );
        Ok(())
    }

    /// Read a batch of packets of the video stream out of the demuxer.
    fn read_packets(&mut self) -> Result<VecDeque<ffmpeg::packet::Packet>, VideoError> {
        let mut batch = VecDeque::new();
        let video_index = self.video_index;
        {
            let ictx = self
                .ictx
                .as_mut()
                .ok_or_else(|| decode_error("the container is not open"))?;
            for (stream, packet) in ictx.packets() {
                if stream.index() != video_index {
                    continue;
                }
                batch.push_back(packet);
                if batch.len() >= PACKET_BATCH {
                    break;
                }
            }
        }
        Ok(batch)
    }

    /// Pull one decoded frame out of the decoder and convert it to RGBA.
    ///
    /// Returns `Ok(None)` when the decoder needs more packets.
    fn receive_frame(&mut self) -> Result<Option<VideoFrame>, VideoError> {
        let Some(decoder) = self.decoder.as_mut() else {
            return Err(decode_error("the decoder is not open"));
        };
        let Some(buffer) = self.frame_buf.as_mut() else {
            return Err(decode_error("the frame buffer is not allocated"));
        };

        match decoder.receive_frame(buffer) {
            Ok(()) => {}
            // The decoder wants more data: the caller has to feed it.
            Err(error) if needs_more_data(&error) => return Ok(None),
            // The decoder is drained: the stream is over.
            Err(error) if is_eof(&error) => {
                self.eof_sent = true;
                return Ok(None);
            }
            Err(error) => {
                return Err(decode_error(format!("cannot decode a frame: {error:?}")));
            }
        }

        // Presentation time of the frame.
        let time = match buffer.pts() {
            Some(pts) => pts as f64 * f64::from(self.time_base.0) / f64::from(self.time_base.1),
            None => {
                let time = self.fallback_time;
                if self.fps > 0.0 {
                    self.fallback_time += 1.0 / self.fps;
                }
                time
            }
        };
        let duration = if self.fps > 0.0 { 1.0 / self.fps } else { 0.0 };

        // Convert the frame to tightly packed RGBA8.
        let (out_width, out_height) = self.out_size;
        let Some(scaler) = self.scaler.as_mut() else {
            return Err(decode_error("the scaler is not available"));
        };
        let Some(output) = self.out_buf.as_mut() else {
            return Err(decode_error("the output buffer is not allocated"));
        };
        scaler
            .run(buffer, output)
            .map_err(|error| decode_error(format!("cannot scale the frame: {error:?}")))?;

        let stride = output.stride(0);
        let row_bytes = out_width as usize * 4;
        let pixels = output.data(0);
        let mut rgba = Vec::with_capacity(row_bytes * out_height as usize);
        for row in 0..out_height as usize {
            let start = row * stride;
            rgba.extend_from_slice(&pixels[start..start + row_bytes]);
        }

        Ok(Some(VideoFrame {
            width: out_width,
            height: out_height,
            time,
            duration,
            rgba: Arc::from(rgba),
        }))
    }
}

impl FrameDecoder for FfmpegBackend {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        self.ensure_open()?;

        let duration = if self.stream_duration > 0 {
            self.stream_duration as f64 * f64::from(self.time_base.0) / f64::from(self.time_base.1)
        } else if self.container_duration > 0 {
            // The container duration is expressed in microseconds.
            self.container_duration as f64 / 1_000_000.0
        } else {
            0.0
        };

        let codec = self.codec_name.clone();
        let has_audio = self
            .ictx
            .as_ref()
            .is_some_and(|context| context.streams().best(ffmpeg::media::Type::Audio).is_some());
        let pixel_format = self
            .src_format
            .descriptor()
            .map(|descriptor| descriptor.name().to_string())
            .unwrap_or_default();
        let frame_count = if duration > 0.0 && self.fps > 0.0 {
            (duration * self.fps).round() as u32
        } else {
            0
        };

        Ok(VideoInfo {
            width: self.src_size.0,
            height: self.src_size.1,
            duration,
            fps: self.fps,
            frame_count,
            codec,
            pixel_format,
            has_audio,
        })
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, VideoError> {
        self.ensure_open()?;

        loop {
            // Try to get a decoded frame first.
            if let Some(frame) = self.receive_frame()? {
                return Ok(Some(frame));
            }

            // The decoder is drained: the stream is over.
            if self.eof_sent {
                return Ok(None);
            }

            // Feed the decoder with the next packet.
            if self.packets.is_empty() {
                self.packets = self.read_packets()?;
            }
            if self.packets.is_empty() {
                // The demuxer is exhausted: flush the decoder.
                let decoder = self
                    .decoder
                    .as_mut()
                    .ok_or_else(|| decode_error("the decoder is not open"))?;
                decoder.send_eof().map_err(|error| {
                    decode_error(format!("cannot flush the decoder: {error:?}"))
                })?;
                self.eof_sent = true;
                continue;
            }

            let packet = self
                .packets
                .pop_front()
                .ok_or_else(|| decode_error("the packet queue is empty"))?;
            let decoder = self
                .decoder
                .as_mut()
                .ok_or_else(|| decode_error("the decoder is not open"))?;
            decoder
                .send_packet(&packet)
                .map_err(|error| decode_error(format!("cannot decode a packet: {error:?}")))?;
        }
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        debug!("rewinding the ffmpeg decoder");

        // Re-opening the demuxer and the decoder is the simplest and most
        // robust way to get back to the first frame.
        self.ictx = None;
        self.decoder = None;
        self.scaler = None;
        self.frame_buf = None;
        self.out_buf = None;
        self.packets.clear();
        self.eof_sent = false;
        self.fallback_time = 0.0;

        self.open_container()
    }

    fn set_output_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.out_size = (width, height);
        if self.decoder.is_some()
            && let Err(error) = self.build_buffers()
        {
            warn!("cannot change the output size of the video: {error}");
        }
    }
}

/// Initialise ffmpeg exactly once.
///
/// ffmpeg keeps global state, so the initialisation has to happen a single
/// time for the whole process.
fn init_ffmpeg() -> Result<(), VideoError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();

    let result = INIT.get_or_init(|| {
        ffmpeg::init().map_err(|error| format!("cannot initialise ffmpeg: {error:?}"))
    });
    match result {
        Ok(()) => Ok(()),
        Err(message) => Err(VideoError::Decode(message.clone())),
    }
}

/// Whether the error means "the decoder needs more packets".
fn needs_more_data(error: &ffmpeg::Error) -> bool {
    matches!(error, ffmpeg::Error::Other { errno } if *errno == ffmpeg::error::EAGAIN)
}

/// Whether the error means "the end of the stream was reached".
fn is_eof(error: &ffmpeg::Error) -> bool {
    matches!(error, ffmpeg::Error::Eof)
}

/// Build a decoding error.
fn decode_error(message: impl Into<String>) -> VideoError {
    VideoError::Decode(message.into())
}
