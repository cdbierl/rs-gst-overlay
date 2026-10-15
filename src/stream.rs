use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use gst::prelude::*;
use gst_video::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

const APPSINK_NAME: &str = "video_sink";

pub(crate) struct VideoFrame {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct StreamHandle {
    state: Arc<Mutex<StreamState>>,
    commands: mpsc::Sender<PipelineCommand>,
}

pub(crate) struct StreamWorker {
    commands: mpsc::Sender<PipelineCommand>,
    join_handle: Option<JoinHandle<()>>,
}

pub(crate) struct StreamStatus {
    pub(crate) message: String,
    pub(crate) playing: bool,
}

#[derive(Default)]
struct StreamState {
    latest_frame: Option<VideoFrame>,
    message: String,
    playing: bool,
}

enum PipelineCommand {
    Play,
    Stop,
    Shutdown,
}

struct PipelineRuntime {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    bus: gst::Bus,
}

impl StreamHandle {
    pub(crate) fn take_latest_frame(&self) -> Option<VideoFrame> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .latest_frame
            .take()
    }

    pub(crate) fn status(&self) -> StreamStatus {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        StreamStatus {
            message: state.message.clone(),
            playing: state.playing,
        }
    }

    pub(crate) fn set_playing(&self, playing: bool) -> bool {
        let command = if playing {
            PipelineCommand::Play
        } else {
            PipelineCommand::Stop
        };
        self.commands.send(command).is_ok()
    }
}

impl Drop for StreamWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(PipelineCommand::Shutdown);
        if let Some(join_handle) = self.join_handle.take()
            && join_handle.join().is_err()
        {
            log::error!("GStreamer video reader thread panicked");
        }
    }
}

pub(crate) fn start(description: String) -> Result<(StreamHandle, StreamWorker), String> {
    let runtime = build_pipeline_runtime(&description)?;
    let state = Arc::new(Mutex::new(StreamState::default()));
    set_message(&state, "Starting pipeline…".to_owned());
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

fn run_worker(
    description: String,
    runtime: PipelineRuntime,
    state: Arc<Mutex<StreamState>>,
    commands: mpsc::Receiver<PipelineCommand>,
) {
    let mut runtime = if start_pipeline(&runtime, &state) {
        Some(runtime)
    } else {
        None
    };

    loop {
        if runtime.is_some() {
            match commands.try_recv() {
                Ok(PipelineCommand::Stop) => {
                    stop_pipeline(&mut runtime, &state, "Pipeline stopped".to_owned());
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
                        state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .latest_frame = Some(frame);
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
                    restart_pipeline(&description, &mut runtime, &state);
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

fn set_playing(state: &Mutex<StreamState>, playing: bool) {
    state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .playing = playing;
}

fn set_message(state: &Mutex<StreamState>, message: String) {
    let changed = {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
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
mod tests {
    use super::{
        PipelineCommand, StreamState, build_pipeline_runtime, frame_from_sample, run_worker,
    };
    use gst::prelude::*;
    use gstreamer as gst;
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
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
                description.to_owned(),
                runtime,
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
                description.to_owned(),
                runtime,
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

    fn test_video_plugins_available() -> bool {
        gst::ElementFactory::find("videotestsrc").is_some()
            && gst::ElementFactory::find("videoconvert").is_some()
            && gst::ElementFactory::find("appsink").is_some()
    }

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
}
