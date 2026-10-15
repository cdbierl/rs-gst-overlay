//! Runs the GStreamer pipeline on a worker thread and transfers video frames to the UI.

use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};

use gst::prelude::*;
use gst_video::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

/// Name required for the pipeline's video appsink element.
const APPSINK_NAME: &str = "video_sink";

/// An owned RGBA video frame ready for upload to an egui texture.
pub(crate) struct VideoFrame {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
}

#[derive(Clone)]
/// Thread-safe controls and status access for the running stream worker.
pub(crate) struct StreamHandle {
    state: Arc<Mutex<StreamState>>,
    commands: mpsc::Sender<PipelineCommand>,
}

/// Owns the worker thread and shuts it down when dropped.
pub(crate) struct StreamWorker {
    commands: mpsc::Sender<PipelineCommand>,
    join_handle: Option<JoinHandle<()>>,
}

/// Snapshot of the user-visible pipeline message and playback state.
pub(crate) struct StreamStatus {
    pub(crate) message: String,
    pub(crate) playing: bool,
}

/// Shared mutable data exchanged between the worker and the UI thread.
#[derive(Default)]
struct StreamState {
    latest_frame: Option<VideoFrame>,
    message: String,
    playing: bool,
}

/// Commands sent from the UI to the pipeline worker.
enum PipelineCommand {
    /// Build a fresh runtime and start playback.
    Play,
    /// Replace the active description and immediately run the selected pipeline.
    Load(String),
    /// Stop the current pipeline runtime.
    Stop,
    /// Shut down the worker thread.
    Shutdown,
}

/// GStreamer objects used while a pipeline is active.
struct PipelineRuntime {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    bus: gst::Bus,
}

/// Acquires stream state, recovering its contents if a previous holder panicked.
fn lock_state(state: &Mutex<StreamState>) -> MutexGuard<'_, StreamState> {
    state.lock().unwrap_or_else(|error| error.into_inner())
}

impl StreamHandle {
    /// Takes the most recently received frame, leaving the slot empty.
    pub(crate) fn take_latest_frame(&self) -> Option<VideoFrame> {
        lock_state(&self.state).latest_frame.take()
    }

    /// Copies the latest status message and playback state.
    pub(crate) fn status(&self) -> StreamStatus {
        let state = lock_state(&self.state);
        StreamStatus {
            message: state.message.clone(),
            playing: state.playing,
        }
    }

    /// Requests playback or stop and reports whether the worker still accepts commands.
    pub(crate) fn set_playing(&self, playing: bool) -> bool {
        let command = if playing {
            PipelineCommand::Play
        } else {
            PipelineCommand::Stop
        };
        self.commands.send(command).is_ok()
    }

    /// Replaces the current pipeline and starts the new description.
    pub(crate) fn load_pipeline(&self, description: String) -> bool {
        self.commands
            .send(PipelineCommand::Load(description))
            .is_ok()
    }
}

impl Drop for StreamWorker {
    /// Signals shutdown and waits for the worker to release its GStreamer resources.
    fn drop(&mut self) {
        let _ = self.commands.send(PipelineCommand::Shutdown);
        if let Some(join_handle) = self.join_handle.take()
            && join_handle.join().is_err()
        {
            log::error!("GStreamer video reader thread panicked");
        }
    }
}

/// Optionally builds a pipeline, starts its worker thread, and returns its handle and owner.
pub(crate) fn start(description: Option<String>) -> Result<(StreamHandle, StreamWorker), String> {
    let runtime = description
        .as_deref()
        .map(build_pipeline_runtime)
        .transpose()?;
    let state = Arc::new(Mutex::new(StreamState::default()));
    if runtime.is_some() {
        set_message(&state, "Starting pipeline…".to_owned());
    } else {
        set_message(&state, "Select a pipeline to start".to_owned());
    }
    let (commands, command_receiver) = mpsc::channel();
    let worker_state = Arc::clone(&state);
    let join_handle = thread::Builder::new()
        .name("gstreamer-video-reader".to_owned())
        .spawn(move || run_worker(description, runtime, worker_state, command_receiver))
        .map_err(|error| format!("could not start video reader: {error}"))?;

    let handle = StreamHandle {
        state,
        commands: commands.clone(),
    };
    let worker = StreamWorker {
        commands,
        join_handle: Some(join_handle),
    };
    Ok((handle, worker))
}

/// Processes pipeline commands, pulls frames, and reports bus errors until shutdown.
fn run_worker(
    mut description: Option<String>,
    runtime: Option<PipelineRuntime>,
    state: Arc<Mutex<StreamState>>,
    commands: mpsc::Receiver<PipelineCommand>,
) {
    let mut runtime = match runtime {
        Some(runtime) if start_pipeline(&runtime, &state) => Some(runtime),
        Some(_) | None => None,
    };

    loop {
        if runtime.is_some() {
            match commands.try_recv() {
                Ok(PipelineCommand::Stop) => {
                    stop_pipeline(&mut runtime, &state, "Pipeline stopped".to_owned());
                    continue;
                }
                Ok(PipelineCommand::Load(new_description)) => {
                    switch_pipeline(new_description, &mut description, &mut runtime, &state);
                    continue;
                }
                Ok(PipelineCommand::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
                Ok(PipelineCommand::Play) | Err(mpsc::TryRecvError::Empty) => {}
            }

            let sample = runtime.as_ref().and_then(|runtime| {
                runtime
                    .appsink
                    .try_pull_sample(gst::ClockTime::from_mseconds(100))
            });
            if let Some(sample) = sample {
                match frame_from_sample(&sample) {
                    Ok(frame) => {
                        lock_state(&state).latest_frame = Some(frame);
                    }
                    Err(error) => {
                        stop_pipeline(&mut runtime, &state, error);
                        continue;
                    }
                }
            }

            let status = runtime.as_ref().and_then(|runtime| {
                runtime
                    .bus
                    .pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
                    .and_then(pipeline_status)
            });
            if let Some(status) = status {
                stop_pipeline(&mut runtime, &state, status);
            }
        } else {
            match commands.recv() {
                Ok(PipelineCommand::Play) => {
                    if let Some(description) = description.as_deref() {
                        restart_pipeline(description, &mut runtime, &state);
                    } else {
                        set_message(&state, "Select a pipeline to start".to_owned());
                    }
                }
                Ok(PipelineCommand::Load(new_description)) => {
                    switch_pipeline(new_description, &mut description, &mut runtime, &state);
                }
                Ok(PipelineCommand::Stop) => {}
                Ok(PipelineCommand::Shutdown) | Err(_) => break,
            }
        }
    }

    if let Some(runtime) = runtime {
        let _ = runtime.pipeline.set_state(gst::State::Null);
    }
    set_playing(&state, false);
}

/// Stops the active pipeline, replaces its description, and starts a fresh runtime.
fn switch_pipeline(
    new_description: String,
    description: &mut Option<String>,
    runtime: &mut Option<PipelineRuntime>,
    state: &Mutex<StreamState>,
) {
    stop_pipeline(runtime, state, "Loading selected pipeline…".to_owned());
    *description = Some(new_description.clone());
    restart_pipeline(&new_description, runtime, state);
}

/// Parses the description and configures its named appsink for single-buffer RGBA delivery.
fn build_pipeline_runtime(description: &str) -> Result<PipelineRuntime, String> {
    let pipeline = crate::pipeline::parse(description)?;
    let element = pipeline
        .by_name(APPSINK_NAME)
        .ok_or_else(|| format!("pipeline must contain an appsink named '{APPSINK_NAME}'"))?;
    let appsink = element
        .downcast::<gst_app::AppSink>()
        .map_err(|_| format!("pipeline element '{APPSINK_NAME}' must be an appsink"))?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| "the GStreamer pipeline has no bus".to_owned())?;

    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .build();
    appsink.set_caps(Some(&caps));
    appsink.set_max_buffers(1);
    appsink.set_drop(true);

    Ok(PipelineRuntime {
        pipeline,
        appsink,
        bus,
    })
}

/// Rebuilds a stopped pipeline from its original description and starts it again.
fn restart_pipeline(
    description: &str,
    runtime: &mut Option<PipelineRuntime>,
    state: &Mutex<StreamState>,
) {
    *runtime = None;
    set_message(state, "Starting pipeline…".to_owned());
    match build_pipeline_runtime(description) {
        Ok(new_runtime) if start_pipeline(&new_runtime, state) => *runtime = Some(new_runtime),
        Ok(_) => {}
        Err(error) => set_message(state, format!("Could not rebuild pipeline: {error}")),
    }
}

/// Clears pending terminal bus messages and transitions the pipeline to playing.
fn start_pipeline(runtime: &PipelineRuntime, state: &Mutex<StreamState>) -> bool {
    while runtime
        .bus
        .pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
        .is_some()
    {}

    match runtime.pipeline.set_state(gst::State::Playing) {
        Ok(_) => {
            set_playing(state, true);
            set_message(state, "Pipeline playing".to_owned());
            true
        }
        Err(error) => {
            let _ = runtime.pipeline.set_state(gst::State::Null);
            set_playing(state, false);
            set_message(state, format!("Could not start pipeline: {error}"));
            false
        }
    }
}

/// Stops and releases the current runtime, then publishes the supplied status message.
fn stop_pipeline(
    runtime: &mut Option<PipelineRuntime>,
    state: &Mutex<StreamState>,
    message: String,
) {
    if let Some(runtime) = runtime.take() {
        let _ = runtime.pipeline.set_state(gst::State::Null);
    }
    set_playing(state, false);
    set_message(state, message);
}

/// Converts GStreamer error and end-of-stream bus messages into displayable status text.
fn pipeline_status(message: gst::Message) -> Option<String> {
    match message.view() {
        gst::MessageView::Error(error) => Some(format!(
            "GStreamer error: {} ({:?})",
            error.error(),
            error.debug()
        )),
        gst::MessageView::Eos(..) => Some("End of stream".to_owned()),
        _ => None,
    }
}

/// Updates the shared playing flag.
fn set_playing(state: &Mutex<StreamState>, playing: bool) {
    lock_state(state).playing = playing;
}

/// Updates and logs the shared status message when its value changes.
fn set_message(state: &Mutex<StreamState>, message: String) {
    let changed = {
        let mut state = lock_state(state);
        if state.message == message {
            false
        } else {
            state.message.clone_from(&message);
            true
        }
    };

    if changed {
        log::info!("Player status: {message}");
    }
}

/// Copies a sample's RGBA pixels into a tightly packed application-owned frame.
fn frame_from_sample(sample: &gst::Sample) -> Result<VideoFrame, String> {
    let caps = sample
        .caps()
        .ok_or_else(|| "appsink received a frame without caps".to_owned())?;
    let info = gst_video::VideoInfo::from_caps(caps)
        .map_err(|error| format!("could not read video frame caps: {error}"))?;
    if info.format() != gst_video::VideoFormat::Rgba {
        return Err(format!(
            "appsink negotiated {:?}, expected RGBA",
            info.format()
        ));
    }

    let width = info.width() as usize;
    let height = info.height() as usize;
    if width == 0 || height == 0 {
        return Err("appsink returned an empty video frame".to_owned());
    }
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| "video frame dimensions are too large".to_owned())?;
    let frame_bytes = row_bytes
        .checked_mul(height)
        .ok_or_else(|| "video frame dimensions are too large".to_owned())?;
    let buffer = sample
        .buffer()
        .ok_or_else(|| "appsink received a sample without a buffer".to_owned())?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
        .map_err(|error| format!("could not map video frame: {error}"))?;
    let plane = frame
        .plane_data(0)
        .map_err(|error| format!("could not read video frame pixels: {error}"))?;
    let stride = frame.plane_stride()[0];
    if stride <= 0 || (stride as usize) < row_bytes {
        return Err("appsink returned an invalid RGBA row stride".to_owned());
    }

    let mut rgba = Vec::with_capacity(frame_bytes);
    for row in 0..height {
        let start = row
            .checked_mul(stride as usize)
            .ok_or_else(|| "video frame dimensions are too large".to_owned())?;
        let end = start
            .checked_add(row_bytes)
            .ok_or_else(|| "video frame dimensions are too large".to_owned())?;
        let pixels = plane
            .get(start..end)
            .ok_or_else(|| "video frame plane is shorter than its dimensions".to_owned())?;
        rgba.extend_from_slice(pixels);
    }

    Ok(VideoFrame {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
/// Exercises appsink frame conversion and pipeline worker restart behavior.
mod tests {
    use super::{
        PipelineCommand, StreamHandle, StreamState, build_pipeline_runtime, frame_from_sample,
        run_worker, start,
    };
    use gst::prelude::*;
    use gstreamer as gst;
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    /// Pulls a generated RGBA frame and checks its dimensions and byte count.
    fn reads_a_rgba_frame_from_appsink() {
        gst::init().unwrap();
        if !test_video_plugins_available() {
            return;
        }

        let runtime = build_pipeline_runtime(
            "videotestsrc num-buffers=1 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink",
        )
        .unwrap();
        runtime.pipeline.set_state(gst::State::Playing).unwrap();
        let sample = runtime
            .appsink
            .try_pull_sample(gst::ClockTime::from_seconds(5));
        let frame_result = sample
            .as_ref()
            .ok_or_else(|| "appsink did not receive a video frame".to_owned())
            .and_then(frame_from_sample);
        runtime.pipeline.set_state(gst::State::Null).unwrap();

        let frame = frame_result.unwrap();
        assert_eq!([frame.width, frame.height], [320, 240]);
        assert_eq!(frame.rgba.len(), frame.width * frame.height * 4);
    }

    #[test]
    /// Verifies that the worker can stop and then restart a live pipeline.
    fn worker_can_stop_and_restart_pipeline() {
        gst::init().unwrap();
        if !test_video_plugins_available() {
            return;
        }

        let description = "videotestsrc is-live=true ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink";
        let runtime = build_pipeline_runtime(description).unwrap();
        let state = Arc::new(Mutex::new(StreamState::default()));
        let (command_sender, command_receiver) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            run_worker(
                Some(description.to_owned()),
                Some(runtime),
                worker_state,
                command_receiver,
            )
        });

        assert!(wait_for_message(&state, "Pipeline playing"));
        command_sender.send(PipelineCommand::Stop).unwrap();
        assert!(wait_for_message(&state, "Pipeline stopped"));
        assert!(!state.lock().unwrap().playing);

        command_sender.send(PipelineCommand::Play).unwrap();
        assert!(wait_for_message(&state, "Pipeline playing"));
        assert!(state.lock().unwrap().playing);

        command_sender.send(PipelineCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    /// Verifies that selecting a different pipeline replaces the active runtime.
    fn worker_can_load_a_selected_pipeline() {
        gst::init().unwrap();
        if !test_video_plugins_available() {
            return;
        }

        let initial_description = "videotestsrc is-live=true ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink";
        let selected_description = "videotestsrc num-buffers=1 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink";
        let runtime = build_pipeline_runtime(initial_description).unwrap();
        let state = Arc::new(Mutex::new(StreamState::default()));
        let (command_sender, command_receiver) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            run_worker(
                Some(initial_description.to_owned()),
                Some(runtime),
                worker_state,
                command_receiver,
            )
        });

        assert!(wait_for_message(&state, "Pipeline playing"));
        command_sender
            .send(PipelineCommand::Load(selected_description.to_owned()))
            .unwrap();
        assert!(wait_for_message(&state, "End of stream"));

        command_sender.send(PipelineCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    /// Opens the worker with no source and starts playback after loading a preset.
    fn worker_can_start_without_an_initial_pipeline() {
        gst::init().unwrap();
        if !test_video_plugins_available() {
            return;
        }

        let (stream, worker) = start(None).unwrap();
        assert_eq!(stream.status().message, "Select a pipeline to start");
        assert!(stream.load_pipeline(
            "videotestsrc is-live=true ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink"
                .to_owned()
        ));
        assert!(wait_for_stream_message(&stream, "Pipeline playing"));

        assert!(stream.set_playing(false));
        assert!(wait_for_stream_message(&stream, "Pipeline stopped"));
        drop(worker);
    }

    #[test]
    /// Verifies that playback can restart after the pipeline reaches end of stream.
    fn worker_can_restart_pipeline_after_end_of_stream() {
        gst::init().unwrap();
        if !test_video_plugins_available() {
            return;
        }

        let description = "videotestsrc is-live=true num-buffers=5 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink";
        let runtime = build_pipeline_runtime(description).unwrap();
        let state = Arc::new(Mutex::new(StreamState::default()));
        let (command_sender, command_receiver) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            run_worker(
                Some(description.to_owned()),
                Some(runtime),
                worker_state,
                command_receiver,
            )
        });

        assert!(wait_for_message(&state, "End of stream"));
        assert!(!state.lock().unwrap().playing);

        command_sender.send(PipelineCommand::Play).unwrap();
        assert!(wait_for_message(&state, "Pipeline playing"));
        assert!(wait_for_message(&state, "End of stream"));

        command_sender.send(PipelineCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    /// Reports whether the source, converter, and sink needed by test pipelines exist.
    fn test_video_plugins_available() -> bool {
        gst::ElementFactory::find("videotestsrc").is_some()
            && gst::ElementFactory::find("videoconvert").is_some()
            && gst::ElementFactory::find("appsink").is_some()
    }

    /// Waits up to five seconds for the worker to publish the expected status message.
    fn wait_for_message(state: &Mutex<StreamState>, expected: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if state.lock().unwrap().message == expected {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits up to five seconds for a worker handle to publish a status message.
    fn wait_for_stream_message(stream: &StreamHandle, expected: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if stream.status().message == expected {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
