//! The worker thread that decodes the frames of a clip.
//!
//! A decoder is a sequential thing: it hands out the frames of the
//! stream one by one.  Doing that on the thread that plays the clip
//! back means that every frame of the video blocks the render loop for
//! as long as it takes to decode it.
//!
//! The worker moves the decoding to a thread of its own: the player
//! sends it a command and waits for the answer.  The frames that are
//! decoded ahead of the playback are kept in a small queue, so that the
//! answer is usually immediate: the render loop then does not wait for
//! the decoding at all.
//!
//! The worker owns the decoder: the player only keeps the frames it has
//! already received (the cache) and the commands it sends.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

use crate::pkg_parser::video_parser::backend::FrameDecoder;
use crate::pkg_parser::video_parser::info::{FrameStatus, VideoError, VideoFrame};

/// How many frames the worker keeps ready.
///
/// The frames are decoded ahead of the playback: the queue holds the
/// frames between the one that is on screen and the horizon the player
/// asked for.  The limit keeps the memory of the queue bounded.
const MAX_READY: usize = 4;

/// A command sent to the worker thread.
enum Command {
    /// The player wants the next frame of the stream.
    ///
    /// The horizon is the time up to which the player may need frames:
    /// the worker decodes ahead of it, so that the frames are ready when
    /// the player asks for them.
    Next { horizon: f64 },
    /// The player wants the stream to start over.
    Rewind,
    /// The player wants the frames at another size.
    OutputSize { width: u32, height: u32 },
    /// The player wants the frames that are never shown to be skipped.
    SkipUnused { enabled: bool },
    /// The player is gone: the worker has to stop.
    Stop,
}

/// The answer of the worker thread.
enum Reply {
    /// The status of the decoder.
    Status(FrameStatus),
    /// The decoder failed.
    Error(String),
}

/// The state of the worker thread.
struct State {
    /// The frames that were decoded ahead of the playback.
    ready: VecDeque<VideoFrame>,
    /// Whether the end of the stream was reached.
    ended: bool,
    /// The error of the decoder, when it failed.
    failure: Option<String>,
    /// Whether the decoder decodes asynchronously.
    ///
    /// A decoder that is not ready yet cannot be decoded ahead of the
    /// playback: it is asked for one frame at a time, exactly like the
    /// player would do it itself.
    asynchronous: bool,
    /// The time up to which the player may need frames.
    horizon: f64,
}

impl State {
    /// Build the state of a worker that decodes nothing yet.
    fn new() -> Self {
        Self {
            ready: VecDeque::new(),
            ended: false,
            failure: None,
            asynchronous: false,
            horizon: f64::NEG_INFINITY,
        }
    }

    /// Decode one frame and put it in the queue.
    ///
    /// The status of the decoder is returned when it did not hand out a
    /// frame (the decoder is not ready yet, or the stream is over).
    fn decode(&mut self, decoder: &mut Box<dyn FrameDecoder + Send>) -> Option<FrameStatus> {
        match decoder.next_frame() {
            Ok(FrameStatus::Frame(frame)) => {
                self.ready.push_back(frame);
                None
            }
            // The decoder is still working on the frame: it cannot be
            // decoded ahead of the playback.
            Ok(FrameStatus::Pending) => {
                self.asynchronous = true;
                Some(FrameStatus::Pending)
            }
            Ok(FrameStatus::End) => {
                self.ended = true;
                Some(FrameStatus::End)
            }
            Err(error) => {
                self.failure = Some(error.to_string());
                None
            }
        }
    }

    /// Whether another frame may be decoded ahead of the playback.
    fn may_decode_ahead(&self) -> bool {
        self.failure.is_none()
            && !self.ended
            && !self.asynchronous
            && self.ready.len() < MAX_READY
            && self
                .ready
                .back()
                .is_none_or(|frame| frame.time <= self.horizon)
    }

    /// Handle a command of the player.
    ///
    /// Returns `false` when the worker has to stop.
    fn handle(
        &mut self,
        command: Command,
        decoder: &mut Box<dyn FrameDecoder + Send>,
        replies: &Sender<Reply>,
    ) -> bool {
        match command {
            Command::Next { horizon } => {
                self.horizon = horizon;

                // The queue is empty: the frame has to be decoded now.
                if self.ready.is_empty()
                    && self.failure.is_none()
                    && !self.ended
                    && let Some(status) = self.decode(decoder)
                {
                    // The decoder is not ready yet (or the stream is
                    // over): the answer is that status.
                    return replies.send(Reply::Status(status)).is_ok();
                }

                let answer = match &self.failure {
                    // The decoder failed: the error is reported again.
                    Some(message) => Reply::Error(message.clone()),
                    None => match self.ready.pop_front() {
                        // A frame was decoded ahead of the playback.
                        Some(frame) => Reply::Status(FrameStatus::Frame(frame)),
                        // The stream is over.
                        None => Reply::Status(FrameStatus::End),
                    },
                };
                replies.send(answer).is_ok()
            }
            Command::Rewind => {
                self.ready.clear();
                self.ended = false;
                self.failure = None;
                self.asynchronous = false;
                if let Err(error) = decoder.rewind() {
                    self.failure = Some(error.to_string());
                }
                true
            }
            Command::OutputSize { width, height } => {
                decoder.set_output_size(width, height);
                true
            }
            Command::SkipUnused { enabled } => {
                decoder.set_frame_skipping(enabled);
                true
            }
            Command::Stop => false,
        }
    }
}

/// The worker thread that owns the decoder of a clip.
///
/// The worker is shut down when it is dropped: the thread is asked to
/// stop and then joined, so that it does not outlive the player.
pub struct Worker {
    /// The commands of the player.
    commands: Sender<Command>,
    /// The answers of the worker.
    replies: Receiver<Reply>,
    /// The thread of the worker: it is joined when the worker is
    /// dropped.
    handle: Option<JoinHandle<()>>,
    /// The name of the backend that runs on the worker.
    name: &'static str,
}

impl Worker {
    /// Start a worker that owns `decoder`.
    ///
    /// The decoder is moved to the new thread: it is used by that thread
    /// and by no other one.
    pub fn new(decoder: Box<dyn FrameDecoder + Send>) -> Self {
        let name = decoder.name();
        let (commands, command_queue) = channel();
        let (reply_queue, replies) = channel();
        let handle = std::thread::Builder::new()
            .name("video-decoder".to_string())
            .spawn(move || run(decoder, command_queue, reply_queue))
            .expect("cannot start the thread that decodes the video");

        Self {
            commands,
            replies,
            handle: Some(handle),
            name,
        }
    }

    /// The name of the backend that runs on the worker.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Ask the worker for the next frame of the stream.
    ///
    /// The `horizon` is the time up to which the player may need frames:
    /// the worker decodes ahead of it, so that the frames are ready when
    /// they are asked for.
    pub fn next_frame(&mut self, horizon: f64) -> Result<FrameStatus, VideoError> {
        self.commands
            .send(Command::Next { horizon })
            .map_err(|_| worker_gone())?;

        match self.replies.recv() {
            Ok(Reply::Status(status)) => Ok(status),
            Ok(Reply::Error(message)) => Err(VideoError::Decode(message)),
            // The worker is gone: it cannot answer any more.
            Err(_) => Err(worker_gone()),
        }
    }

    /// Ask the worker to start the stream over.
    ///
    /// The command is handled before the next one, so the stream really
    /// starts over when the next frame is asked for.
    pub fn rewind(&mut self) -> Result<(), VideoError> {
        self.commands
            .send(Command::Rewind)
            .map_err(|_| worker_gone())
    }

    /// Ask the worker for frames of another size.
    pub fn set_output_size(&mut self, width: u32, height: u32) {
        let _ = self.commands.send(Command::OutputSize { width, height });
    }

    /// Ask the worker to skip the frames that are never shown.
    pub fn set_frame_skipping(&mut self, enabled: bool) {
        let _ = self.commands.send(Command::SkipUnused { enabled });
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // The worker is asked to stop...
        let _ = self.commands.send(Command::Stop);
        // ... and the thread is joined, so that it does not outlive the
        // player (and does not keep the process alive).
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The loop of the worker thread.
fn run(
    mut decoder: Box<dyn FrameDecoder + Send>,
    commands: Receiver<Command>,
    replies: Sender<Reply>,
) {
    let mut state = State::new();

    loop {
        // The commands that are waiting are handled first: the player
        // must not wait for the frames that are decoded ahead.
        let mut stopped = false;
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    if !state.handle(command, &mut decoder, &replies) {
                        stopped = true;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                // The player is gone: the worker stops.
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if stopped {
            return;
        }

        // Decode the frames the player will ask for next, so that they
        // are ready when it asks for them.  The commands are checked
        // in between: the player must not wait for the frames that come
        // after the one it asks for.
        if state.may_decode_ahead() {
            state.decode(&mut decoder);
            continue;
        }

        // There is nothing to do: wait for a command.
        match commands.recv() {
            Ok(command) => {
                if !state.handle(command, &mut decoder, &replies) {
                    return;
                }
            }
            // The player is gone: the worker stops.
            Err(_) => return,
        }
    }
}

/// The error of a worker that is not there any more.
fn worker_gone() -> VideoError {
    VideoError::Decode("the thread that decodes the video is gone".to_string())
}
