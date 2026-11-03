//! The browser backends: the media element of the page and the WebCodecs
//! `ImageDecoder`.
//!
//! Both decoders let the *browser* do the decoding; this module only
//! moves the pixels around:
//!
//! * [`WebVideoBackend`] drives an HTML `<video>` element.  The element
//!   demuxes the container (mp4, webm, …) **and** decodes the video,
//!   which is exactly what the WebCodecs `VideoDecoder` does not do: the
//!   codec API takes encoded chunks and knows nothing about the
//!   containers, so feeding it an `.mp4` would mean writing an MP4
//!   demuxer.  The frames are read back **synchronously** with
//!   `drawImage` into an `OffscreenCanvas` followed by `getImageData`.
//! * [`WebImageBackend`] uses the WebCodecs `ImageDecoder`, which decodes
//!   a whole encoded image (png, jpeg, webp, …) in one go.  That API is
//!   asynchronous: the frame shows up a few microtasks later, so the
//!   backend reports [`FrameStatus::Pending`] until the browser delivered
//!   it.
//!
//! # Audio
//!
//! Audio is deliberately **not** handled here.  The browser decodes the
//! sound of a video for the *playback* path (`decodeAudioData`,
//! `AudioDecoder`), which is not a source of frames: the crate decodes
//! pictures, and a renderer that wants the sound of a clip has to play
//! the media element itself and route it into an `AudioContext`.
//!
//! # Asynchronous decoders
//!
//! The browser works on its own schedule: the media element decodes the
//! next frame while the page runs, and the `ImageDecoder` resolves a
//! promise when it is done.  The backends therefore report
//! [`FrameStatus::Pending`] while they are waiting, and the caller has to
//! ask again a bit later.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use js_sys::{Array, Function, Object, Uint8Array};
use log::{debug, warn};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, HtmlVideoElement, OffscreenCanvas, OffscreenCanvasRenderingContext2d,
    Url, VideoFrameCopyToOptions, VideoPixelFormat,
};
// The frame of the WebCodecs is not the frame of the crate: the browser
// hands its own type over, which is converted into the RGBA buffer of a
// [`VideoFrame`].
use web_sys::VideoFrame as WebVideoFrame;

use crate::pkg_parser::video_parser::backend::FrameDecoder;
use crate::pkg_parser::video_parser::info::{
    FramePixels, FrameStatus, VideoError, VideoFrame, VideoInfo,
};
use crate::pkg_parser::video_parser::mp4_meta::{Mp4Meta, probe_mp4};

/// The `readyState` of a media element that has a frame to show.
const HAVE_CURRENT_DATA: u16 = 2;

/// The tolerance used when comparing the playback positions of two frames.
const TIME_EPSILON: f64 = 1e-6;

/// A video decoder that drives the browser's own media pipeline.
///
/// The video element demuxes and decodes the file; the frames are read back
/// from a canvas. Everything the renderer needs is synchronous.
///
/// The element is created *detached* (it is not added to the page): a
/// detached element still demuxes and decodes, and it does not disturb
/// the layout of the page.  It is muted and inline, so that the browser
/// is allowed to play it without a user gesture.
pub struct WebVideoBackend {
    /// The mime type of the container (e.g. `"video/mp4"`).
    mime: String,
    /// The media element that demuxes and decodes the container.
    video: HtmlVideoElement,
    /// The canvas the frames are drawn into, when the browser has one.
    canvas: Option<Canvas>,
    /// The blob URL the media element reads from; revoked on drop.
    object_url: Option<String>,
    /// The metadata of the mp4 header, when the container is an mp4.
    meta: Option<Mp4Meta>,
    /// The playback position of the frame that was handed out last.
    last_time: f64,
    /// Whether a frame was handed out already.
    delivered: bool,
    /// Whether the media element was asked to play.
    ///
    /// The flag is shared with the task that waits for the promise of
    /// `play()`: a browser that refuses to play the video (because of its
    /// autoplay policy, for instance) clears the flag, so that the
    /// playback can be tried again later.
    playing: Rc<Cell<bool>>,
    /// Whether the end of the stream was reached.
    ended: bool,
    /// Whether a seek is in progress.
    ///
    /// The flag is shared with the `seeked` event handler of the media
    /// element: the frames are only valid again once the seek is over.
    seeking: Rc<Cell<bool>>,
    /// The handler of the `seeked` event.
    ///
    /// It is never read, but it has to be kept alive: a `Closure` that is
    /// dropped invalidates the JavaScript function behind it.
    _seeked_handler: Closure<dyn FnMut()>,
    /// The size the frames have to be delivered in, when the caller asked
    /// for it.
    output_size: Option<(u32, u32)>,
}

/// The canvas the frames of the media element are drawn into.
struct Canvas {
    /// The canvas itself.
    canvas: OffscreenCanvas,
    /// Its two dimensional context.
    context: OffscreenCanvasRenderingContext2d,
}

impl WebVideoBackend {
    /// Create a backend for the given container bytes.
    ///
    /// `mime` is the type of the container (`"video/mp4"`, `"video/webm"`,
    /// …): the browser needs it to pick the demuxer and the decoder.
    ///
    /// The function cannot fail: a browser always has a media element and
    /// a canvas.  When the blob URL of the payload cannot be built, the
    /// problem is logged and the element simply has no source.
    pub fn new(bytes: Arc<[u8]>, mime: &str) -> Self {
        let video = create_video_element().expect("the browser has no document");

        // The element has to be muted: a browser refuses to play a video
        // with sound without a user gesture, and the frames are wanted
        // right away.
        video.set_muted(true);
        // `playsInline` is not bound by `web-sys`, so it is set through
        // the reflection API of the DOM.
        let _ = js_sys::Reflect::set(&video, &JsValue::from_str("playsInline"), &JsValue::TRUE);
        // The whole file is needed: the frames are pulled out of the
        // element while it plays.
        video.set_preload("auto");

        // The payload is handed to the browser as a blob: a media element
        // cannot read from the memory of the wasm module.
        let object_url = match create_blob_url(&bytes, mime) {
            Ok(url) => Some(url),
            Err(error) => {
                warn!("cannot create the blob URL of the video: {error}");
                None
            }
        };
        if let Some(url) = &object_url {
            video.set_src(url);
        }

        // The metadata of the mp4 header is the only source of the size of
        // the video before the element loaded its own metadata.
        let meta = probe_mp4(&bytes);
        let (width, height) = meta.map(|meta| (meta.width, meta.height)).unwrap_or((1, 1));
        let canvas = create_canvas(width, height);

        // The `seeked` event tells when a seek is over.
        let seeking = Rc::new(Cell::new(false));
        let flag = Rc::clone(&seeking);
        let seeked_handler = Closure::wrap(Box::new(move || {
            flag.set(false);
        }) as Box<dyn FnMut()>);
        video.set_onseeked(Some(seeked_handler.as_ref().unchecked_ref()));

        Self {
            mime: mime.to_string(),
            video,
            canvas,
            object_url,
            meta,
            last_time: 0.0,
            delivered: false,
            playing: Rc::new(Cell::new(false)),
            ended: false,
            seeking,
            _seeked_handler: seeked_handler,
            output_size: None,
        }
    }

    /// The metadata of the mp4 header, when the container is an mp4.
    fn meta(&self) -> Option<Mp4Meta> {
        self.meta
    }

    /// Ask the media element to play.
    ///
    /// The element is muted, so the browser is allowed to start it without
    /// a user gesture.
    fn start_playback(&mut self) {
        if self.playing.get() {
            return;
        }
        self.playing.set(true);

        let playing = Rc::clone(&self.playing);
        match self.video.play() {
            Ok(promise) => {
                // The promise is rejected when the browser refuses to play
                // the video (a broken file, a codec it does not know, …).
                // The flag is cleared, so that the playback can be tried
                // again later.
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(error) = JsFuture::from(promise).await {
                        warn!("the browser refused to play the video: {error:?}");
                        playing.set(false);
                    }
                });
            }
            Err(error) => {
                warn!("cannot play the video: {error:?}");
                self.playing.set(false);
            }
        }
    }

    /// Draw the current frame of the media element into the canvas and read
    /// the pixels back.
    fn read_frame(&mut self, time: f64) -> Result<VideoFrame, VideoError> {
        let canvas = self.canvas.as_ref().ok_or_else(|| {
            VideoError::Unsupported("the browser has no OffscreenCanvas".to_string())
        })?;

        // The frames are delivered in the native resolution of the video,
        // unless the caller asked for another size.
        let (width, height) = self
            .output_size
            .unwrap_or_else(|| (self.video.video_width(), self.video.video_height()));
        if width == 0 || height == 0 {
            return Err(VideoError::Decode(
                "the video has no dimensions (yet)".to_string(),
            ));
        }

        // The canvas has to follow the size of the frames.
        if canvas.canvas.width() != width || canvas.canvas.height() != height {
            canvas.canvas.set_width(width);
            canvas.canvas.set_height(height);
        }

        canvas
            .context
            .draw_image_with_html_video_element_and_dw_and_dh(
                &self.video,
                0.0,
                0.0,
                f64::from(width),
                f64::from(height),
            )
            .map_err(|error| {
                VideoError::Decode(format!("cannot draw the frame of the video: {error:?}"))
            })?;

        // The pixels are read back synchronously: `getImageData` is the
        // only way to get them out of the canvas.
        let image = canvas
            .context
            .get_image_data(0.0, 0.0, f64::from(width), f64::from(height))
            .map_err(|error| {
                VideoError::Decode(format!("cannot read the pixels of the canvas: {error:?}"))
            })?;

        Ok(VideoFrame {
            width,
            height,
            time,
            // The duration of a frame is not known: the media element
            // does not tell how long the current frame stays on screen.
            duration: 0.0,
            pixels: FramePixels::Rgba(Arc::from(image.data().to_vec())),
        })
    }
}

impl FrameDecoder for WebVideoBackend {
    fn name(&self) -> &'static str {
        "web-video"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        // The size and the duration of the video are known twice: the mp4
        // header carries them right away, the media element knows them
        // once its metadata was loaded.  The element wins, because it is
        // the one that decodes the file.
        let (mut width, mut height, mut duration) = self
            .meta()
            .map(|meta| (meta.width, meta.height, meta.duration))
            .unwrap_or((0, 0, 0.0));

        if self.video.video_width() > 0 {
            width = self.video.video_width();
        }
        if self.video.video_height() > 0 {
            height = self.video.video_height();
        }
        let element_duration = self.video.duration();
        if element_duration.is_finite() && element_duration > 0.0 {
            duration = element_duration;
        }

        Ok(VideoInfo {
            width,
            height,
            duration,
            // The frame rate is not exposed by the media element.
            fps: 0.0,
            frame_count: 0,
            codec: self.mime.clone(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
        if self.ended {
            return Ok(FrameStatus::End);
        }

        // The metadata has to be loaded before the element has a frame to
        // show.
        if self.video.ready_state() < HAVE_CURRENT_DATA {
            self.start_playback();
            return Ok(FrameStatus::Pending);
        }

        // The frames are only valid again once the seek is over.
        if self.seeking.get() {
            return Ok(FrameStatus::Pending);
        }

        let time = self.video.current_time();
        if self.delivered && (time - self.last_time).abs() < TIME_EPSILON {
            // The element did not produce a new frame yet.
            self.start_playback();
            return Ok(FrameStatus::Pending);
        }

        let frame = self.read_frame(time)?;
        self.last_time = time;
        self.delivered = true;

        // The end of the stream is only reported once the last frame was
        // handed out.
        if self.video.ended() {
            debug!("the browser reached the end of the video");
            self.ended = true;
        }

        Ok(FrameStatus::Frame(frame))
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        debug!("rewinding the web video decoder");

        self.delivered = false;
        self.last_time = 0.0;
        self.ended = false;

        // The element has to seek back to the beginning.  The `seeked`
        // event tells when it is over; the flag is only raised when the
        // element really has to move, because a seek to the position the
        // element already sits at does not fire the event.
        if self.video.current_time() > TIME_EPSILON {
            self.seeking.set(true);
            self.video.set_current_time(0.0);
        }

        // The element may have been paused (at the end of the stream, for
        // instance): the playback has to start again.
        self.start_playback();

        Ok(())
    }

    fn set_output_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.output_size = Some((width, height));
    }
}

impl Drop for WebVideoBackend {
    fn drop(&mut self) {
        // The blob URL keeps the payload alive; it has to be released when
        // the backend is dropped.
        if let Some(url) = self.object_url.take() {
            if let Err(error) = Url::revoke_object_url(&url) {
                warn!("cannot revoke the blob URL of the video: {error:?}");
            }
        }
    }
}

/// A still image decoder that uses the browser's WebCodecs `ImageDecoder`.
///
/// The `ImageDecoder` decodes a whole encoded image (png, jpeg, webp, …)
/// in one go.  It is asynchronous: the browser hands the frame over a few
/// microtasks later, so the backend reports [`FrameStatus::Pending`] until
/// then.
pub struct WebImageBackend {
    /// The raw image payload.
    bytes: Arc<[u8]>,
    /// The mime type of the payload (e.g. `"image/png"`).
    mime: String,
    /// The state of the decoding, shared with the background task.
    state: Rc<RefCell<DecodeState>>,
    /// Whether the frame was handed out already.
    delivered: bool,
}

/// The state of the decoding of an image.
#[derive(Debug, Default)]
enum DecodeState {
    /// The decoding was not started yet.
    #[default]
    Idle,
    /// The browser is decoding the image.
    Decoding,
    /// The image was decoded.
    Ready(DecodedImage),
    /// The decoding failed.
    Failed(String),
}

/// An image that was decoded by the browser.
#[derive(Debug, Clone)]
struct DecodedImage {
    /// The width of the image, in pixels.
    width: u32,
    /// The height of the image, in pixels.
    height: u32,
    /// The tightly packed RGBA8 pixels of the image.
    rgba: Arc<[u8]>,
}

impl WebImageBackend {
    /// Create a backend for the given image bytes.
    ///
    /// `mime` is the type of the image (`"image/png"`, `"image/jpeg"`,
    /// …): the browser needs it to pick the decoder.
    ///
    /// The decoding starts right away; the frame is picked up by
    /// [`next_frame`](FrameDecoder::next_frame) as soon as the browser is
    /// done.
    pub fn new(bytes: Arc<[u8]>, mime: &str) -> Self {
        let backend = Self {
            bytes,
            mime: mime.to_string(),
            state: Rc::new(RefCell::new(DecodeState::Idle)),
            delivered: false,
        };
        backend.start_decoding();
        backend
    }

    /// Start the decoding of the image in the background.
    fn start_decoding(&self) {
        *self.state.borrow_mut() = DecodeState::Decoding;

        let state = Rc::clone(&self.state);
        let bytes = self.bytes.clone();
        let mime = self.mime.clone();

        wasm_bindgen_futures::spawn_local(async move {
            match decode_image(&bytes, &mime).await {
                Ok(image) => {
                    debug!(
                        "the browser decoded a {}x{} image",
                        image.width, image.height
                    );
                    *state.borrow_mut() = DecodeState::Ready(image);
                }
                Err(error) => {
                    warn!("the browser cannot decode the image: {error}");
                    *state.borrow_mut() = DecodeState::Failed(error.to_string());
                }
            }
        });
    }
}

impl FrameDecoder for WebImageBackend {
    fn name(&self) -> &'static str {
        "web-image"
    }

    fn probe(&mut self) -> Result<VideoInfo, VideoError> {
        // The size of the image is only known once the browser decoded it.
        let (width, height) = match &*self.state.borrow() {
            DecodeState::Ready(image) => (image.width, image.height),
            _ => (0, 0),
        };

        Ok(VideoInfo {
            width,
            height,
            // A still image has no duration: it is on screen forever.
            duration: 0.0,
            fps: 0.0,
            frame_count: 1,
            codec: self.mime.clone(),
            pixel_format: "rgba8".to_string(),
            has_audio: false,
        })
    }

    fn next_frame(&mut self) -> Result<FrameStatus, VideoError> {
        let status = {
            let state = self.state.borrow();
            match &*state {
                DecodeState::Idle | DecodeState::Decoding => FrameStatus::Pending,
                DecodeState::Ready(image) => {
                    if self.delivered {
                        // A still image is a stream with a single frame.
                        FrameStatus::End
                    } else {
                        FrameStatus::Frame(VideoFrame {
                            width: image.width,
                            height: image.height,
                            time: 0.0,
                            duration: 0.0,
                            pixels: FramePixels::Rgba(image.rgba.clone()),
                        })
                    }
                }
                DecodeState::Failed(message) => {
                    return Err(VideoError::Decode(message.clone()));
                }
            }
        };

        if matches!(status, FrameStatus::Frame(_)) {
            self.delivered = true;
        }
        Ok(status)
    }

    fn rewind(&mut self) -> Result<(), VideoError> {
        debug!("rewinding the web image decoder");

        self.delivered = false;

        // The pixels are kept around, so the frame can be handed out
        // again without decoding the image a second time.  A decoding
        // that failed (or that never started) is started again though.
        let restart = matches!(
            &*self.state.borrow(),
            DecodeState::Idle | DecodeState::Failed(_)
        );
        if restart {
            self.start_decoding();
        }
        Ok(())
    }
}

/// Decode an encoded image with the WebCodecs `ImageDecoder`.
///
/// The typed bindings of `web-sys` for the `ImageDecoder` are only
/// generated when the whole dependency tree is compiled with
/// `--cfg=web_sys_unstable_apis`, which needs a global `RUSTFLAGS`
/// setting; the constructor is therefore called through the reflection
/// API of JavaScript.  The browser API is exactly the same, only the
/// binding is not generated.
async fn decode_image(bytes: &[u8], mime: &str) -> Result<DecodedImage, VideoError> {
    let decoder = create_image_decoder(bytes, mime)?;

    // `decode()` returns a promise that resolves with the decoded frame.
    let promise = call_method(&decoder, "decode")?;
    let promise: js_sys::Promise = promise.unchecked_into();
    let result = JsFuture::from(promise).await.map_err(|error| {
        VideoError::Decode(format!("the browser cannot decode the image: {error:?}"))
    })?;

    // The result holds the frame of the image.
    let image = js_sys::Reflect::get(&result, &JsValue::from_str("image")).map_err(|error| {
        VideoError::Decode(format!("the decoded image has no frame: {error:?}"))
    })?;
    let frame: WebVideoFrame = image.unchecked_into();

    let width = frame.display_width();
    let height = frame.display_height();
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

    // The frame is copied into a RGBA buffer: the format of the frame
    // depends on the decoder, but RGBA is what the crate expects.
    let options = VideoFrameCopyToOptions::new();
    options.set_format(VideoPixelFormat::Rgba);
    // The pixels are copied synchronously; the promise only tells whether
    // the copy worked.
    let copied = frame.copy_to_with_u8_slice_and_options(&mut pixels, &options);
    JsFuture::from(copied)
        .await
        .map_err(|error| VideoError::Decode(format!("cannot copy the decoded image: {error:?}")))?;
    // The frame holds a buffer of the browser: it has to be released.
    frame.close();

    Ok(DecodedImage {
        width,
        height,
        rgba: Arc::from(pixels),
    })
}

/// Create the WebCodecs `ImageDecoder` of an encoded image.
///
/// The constructor is called through the reflection API of JavaScript,
/// because the typed bindings of `web-sys` are only generated with the
/// `web_sys_unstable_apis` configuration.
fn create_image_decoder(bytes: &[u8], mime: &str) -> Result<Object, VideoError> {
    let global = js_sys::global();
    let constructor =
        js_sys::Reflect::get(&global, &JsValue::from_str("ImageDecoder")).map_err(|error| {
            VideoError::Unsupported(format!("the browser has no ImageDecoder: {error:?}"))
        })?;
    let constructor = constructor
        .dyn_ref::<Function>()
        .ok_or_else(|| VideoError::Unsupported("the browser has no ImageDecoder".to_string()))?;

    // The init dictionary of the decoder: the payload and its type.
    let init = Object::new();
    let data = Uint8Array::from(bytes);
    js_sys::Reflect::set(&init, &JsValue::from_str("data"), data.as_ref()).map_err(|error| {
        VideoError::Decode(format!(
            "cannot build the init of the ImageDecoder: {error:?}"
        ))
    })?;
    js_sys::Reflect::set(&init, &JsValue::from_str("type"), &JsValue::from_str(mime)).map_err(
        |error| {
            VideoError::Decode(format!(
                "cannot build the init of the ImageDecoder: {error:?}"
            ))
        },
    )?;

    let decoder = constructor.call1(&JsValue::NULL, &init).map_err(|error| {
        VideoError::Decode(format!("cannot create the ImageDecoder: {error:?}"))
    })?;

    Ok(decoder.into())
}

/// Call a method of a JavaScript object and return its result.
fn call_method(object: &Object, name: &str) -> Result<JsValue, VideoError> {
    let method = js_sys::Reflect::get(object, &JsValue::from_str(name)).map_err(|error| {
        VideoError::Decode(format!("the browser has no `{name}` method: {error:?}"))
    })?;
    let method = method
        .dyn_ref::<Function>()
        .ok_or_else(|| VideoError::Decode(format!("the `{name}` method is not a function")))?;

    method
        .call0(object)
        .map_err(|error| VideoError::Decode(format!("the `{name}` method failed: {error:?}")))
}

/// Create the blob URL of a payload.
///
/// A media element (and the `ImageDecoder`) cannot read from the memory
/// of the wasm module: the payload is handed to the browser as a blob and
/// the blob is addressed by an URL.
fn create_blob_url(bytes: &[u8], mime: &str) -> Result<String, VideoError> {
    let parts = Array::of1(&Uint8Array::from(bytes));
    let options = BlobPropertyBag::new();
    options.set_type(mime);

    let blob = Blob::new_with_u8_array_sequence_and_options(parts.as_ref(), &options).map_err(
        |error| VideoError::Decode(format!("cannot build the blob of the payload: {error:?}")),
    )?;

    Url::create_object_url_with_blob(&blob).map_err(|error| {
        VideoError::Decode(format!(
            "cannot create the blob URL of the payload: {error:?}"
        ))
    })
}

/// Create the media element the frames are read from.
///
/// The element is *detached*: it is not added to the page, so it does not
/// disturb the layout of the application.  A detached element still
/// demuxes and decodes the file it is given.
fn create_video_element() -> Result<HtmlVideoElement, VideoError> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| VideoError::Unsupported("the browser has no document".to_string()))?;

    let element = document.create_element("video").map_err(|error| {
        VideoError::Unsupported(format!("cannot create a video element: {error:?}"))
    })?;

    Ok(element.unchecked_into::<HtmlVideoElement>())
}

/// Create the canvas the frames are drawn into.
fn create_canvas(width: u32, height: u32) -> Option<Canvas> {
    let canvas = OffscreenCanvas::new(width.max(1), height.max(1)).ok()?;
    let context = canvas.get_context("2d").ok().flatten()?;
    let context = context
        .dyn_into::<OffscreenCanvasRenderingContext2d>()
        .ok()?;

    Some(Canvas { canvas, context })
}
