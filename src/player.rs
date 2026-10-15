use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use gst::prelude::*;
use gst_video::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

use crate::overlay::{CrosshairStyle, paint_crosshair};

const APPSINK_NAME: &str = "video_sink";
const MAX_ZOOM: f32 = 8.0;

struct VideoFrame {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

#[derive(Default)]
struct SharedState {
    latest_frame: Option<VideoFrame>,
    message: String,
}

pub fn play(pipeline: gst::Pipeline) -> Result<(), String> {
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

    let state = Arc::new(Mutex::new(SharedState::default()));
    set_message(&state, "Starting pipeline…".to_owned());
    let stop = Arc::new(AtomicBool::new(false));
    let worker_state = Arc::clone(&state);
    let worker_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("gstreamer-video-reader".to_owned())
        .spawn(move || stream_frames(pipeline, appsink, bus, worker_state, worker_stop))
        .map_err(|error| format!("could not start video reader: {error}"))?;

    let app_state = Arc::clone(&state);
    let result = eframe::run_native(
        "rs_gst_overlay",
        eframe::NativeOptions::default(),
        Box::new(move |_creation_context| Ok(Box::new(VideoApp::new(app_state)))),
    );

    stop.store(true, Ordering::Relaxed);
    if worker.join().is_err() {
        log::error!("GStreamer video reader thread panicked");
    }

    result.map_err(|error| format!("could not run egui window: {error}"))
}

fn stream_frames(
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    bus: gst::Bus,
    state: Arc<Mutex<SharedState>>,
    stop: Arc<AtomicBool>,
) {
    if let Err(error) = pipeline.set_state(gst::State::Playing) {
        set_message(&state, format!("Could not start pipeline: {error}"));
        let _ = pipeline.set_state(gst::State::Null);
        return;
    }
    set_message(&state, "Pipeline playing".to_owned());

    while !stop.load(Ordering::Relaxed) {
        if let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
            match frame_from_sample(&sample) {
                Ok(frame) => {
                    state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .latest_frame = Some(frame);
                }
                Err(error) => {
                    set_message(&state, error);
                    break;
                }
            }
        }

        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos]) {
            match message.view() {
                gst::MessageView::Error(error) => {
                    set_message(
                        &state,
                        format!("GStreamer error: {} ({:?})", error.error(), error.debug()),
                    );
                    break;
                }
                gst::MessageView::Eos(..) => {
                    set_message(&state, "End of stream".to_owned());
                    break;
                }
                _ => {}
            }
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
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

fn set_message(state: &Mutex<SharedState>, message: String) {
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

struct VideoApp {
    state: Arc<Mutex<SharedState>>,
    texture: Option<TextureHandle>,
    image_size: [usize; 2],
    zoom: f32,
    show_crosshair: bool,
}

impl VideoApp {
    fn new(state: Arc<Mutex<SharedState>>) -> Self {
        Self {
            state,
            texture: None,
            image_size: [0, 0],
            zoom: 1.0,
            show_crosshair: true,
        }
    }

    fn update_texture(&mut self, context: &egui::Context) {
        let frame = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .latest_frame
            .take();
        let Some(frame) = frame else {
            return;
        };

        let image =
            egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        if self.image_size != [frame.width, frame.height] {
            self.image_size = [frame.width, frame.height];
            self.texture = Some(context.load_texture("video-frame", image, TextureOptions::LINEAR));
        } else if let Some(texture) = self.texture.as_mut() {
            texture.set(image, TextureOptions::LINEAR);
        }
    }
}

impl eframe::App for VideoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_texture(ui.ctx());
        let message = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .message
            .clone();

        ui.vertical(|ui| {
            egui::Frame::NONE.inner_margin(8.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.show_crosshair, "Crosshair");
                    ui.add(
                        egui::Slider::new(&mut self.zoom, 1.0..=MAX_ZOOM)
                            .text("Zoom")
                            .suffix("×"),
                    );
                    if ui.button("Reset").clicked() {
                        self.zoom = 1.0;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(&message).truncate());
                    });
                });
            });
            ui.separator();
            let Some(texture_id) = self.texture.as_ref().map(TextureHandle::id) else {
                ui.centered_and_justified(|ui| ui.label(message));
                return;
            };

            let available = ui.available_rect_before_wrap();
            let image_width = self.image_size[0] as f32;
            let image_height = self.image_size[1] as f32;
            let fit_scale = (available.width() / image_width)
                .min(available.height() / image_height)
                .max(0.0);
            let image_size = Vec2::new(image_width, image_height) * fit_scale;
            let image_rect = Rect::from_center_size(available.center(), image_size);

            if image_rect
                .contains(ui.input(|input| input.pointer.hover_pos().unwrap_or(Pos2::ZERO)))
            {
                let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(1.0, MAX_ZOOM);
                }
            }

            let crop = (1.0 - self.zoom.recip()) / 2.0;
            let uv = Rect::from_min_max(Pos2::new(crop, crop), Pos2::new(1.0 - crop, 1.0 - crop));
            let painter = ui.painter().with_clip_rect(image_rect);
            painter.image(texture_id, image_rect, uv, Color32::WHITE);
            if self.show_crosshair {
                paint_crosshair(&painter, image_rect, CrosshairStyle::default());
            }
            ui.allocate_rect(image_rect, Sense::hover());
        });

        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

#[cfg(test)]
mod tests {
    use super::frame_from_sample;
    use gst::prelude::*;
    use gstreamer as gst;
    use gstreamer_app as gst_app;

    #[test]
    fn reads_a_rgba_frame_from_appsink() {
        gst::init().unwrap();
        if gst::ElementFactory::find("videotestsrc").is_none()
            || gst::ElementFactory::find("videoconvert").is_none()
            || gst::ElementFactory::find("appsink").is_none()
        {
            return;
        }

        let pipeline = gst::parse::launch(
            "videotestsrc num-buffers=1 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let appsink = pipeline
            .by_name("video_sink")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();

        let sample = appsink.try_pull_sample(gst::ClockTime::from_seconds(5));
        let frame_result = sample
            .as_ref()
            .ok_or_else(|| "appsink did not receive a video frame".to_owned())
            .and_then(frame_from_sample);
        pipeline.set_state(gst::State::Null).unwrap();

        let frame = frame_result.unwrap();
        assert_eq!([frame.width, frame.height], [320, 240]);
        assert_eq!(frame.rgba.len(), frame.width * frame.height * 4);
    }
}
