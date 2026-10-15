use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use egui_material3::material_symbol::material_symbol_text;
use egui_material3::{MaterialIconButton, MaterialSlider, MaterialSwitch};
use gst::prelude::*;
use gst_video::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

use crate::overlay::{CrosshairStyle, paint_crosshair};

const APPSINK_NAME: &str = "video_sink";
const MAX_ZOOM: f32 = 10.0;
const TOOLBAR_HEIGHT: f32 = 48.0;

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
        Box::new(move |creation_context| {
            egui_material3::theme::setup_local_theme(None);
            egui_material3::theme::load_themes();
            egui_material3::theme::setup_local_fonts_from_bytes(
                "MaterialSymbolsRounded",
                include_bytes!("../assets/MaterialSymbolsRounded-Regular.ttf"),
            );
            egui_material3::theme::load_fonts(&creation_context.egui_ctx);
            egui_material3::theme::update_window_background(&creation_context.egui_ctx);
            Ok(Box::new(VideoApp::new(app_state)))
        }),
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
    rotation_quarters: u8,
    flip_horizontal: bool,
    flip_vertical: bool,
}

impl VideoApp {
    fn new(state: Arc<Mutex<SharedState>>) -> Self {
        Self {
            state,
            texture: None,
            image_size: [0, 0],
            zoom: 1.0,
            show_crosshair: true,
            rotation_quarters: 0,
            flip_horizontal: false,
            flip_vertical: false,
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
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_texture(context);
        let message = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .message
            .clone();

        egui::CentralPanel::default().show(context, |ui| {
            ui.vertical(|ui| {
                egui::Frame::NONE.inner_margin(8.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    let toolbar_size = Vec2::new(ui.available_width(), TOOLBAR_HEIGHT);
                    ui.allocate_ui_with_layout(
                        toolbar_size,
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label("Crosshair");
                            ui.add(MaterialSwitch::new(&mut self.show_crosshair));
                            ui.label("Zoom");
                            ui.add(
                                MaterialSlider::new(&mut self.zoom, 1.0..=MAX_ZOOM).width(100.0),
                            );
                            if ui
                                .add(MaterialIconButton::standard(material_symbol_text(
                                    "rotate_90_degrees_cw",
                                )))
                                .on_hover_text("Rotate video 90° clockwise")
                                .clicked()
                            {
                                self.rotation_quarters = (self.rotation_quarters + 1) % 4;
                            }
                            ui.add(MaterialIconButton::toggle(
                                material_symbol_text("swap_horiz"),
                                &mut self.flip_horizontal,
                            ))
                            .on_hover_text("Flip video horizontally");
                            ui.add(MaterialIconButton::toggle(
                                material_symbol_text("swap_vert"),
                                &mut self.flip_vertical,
                            ))
                            .on_hover_text("Flip video vertically");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add(egui::Label::new(&message).truncate());
                                },
                            );
                        },
                    );
                });
                ui.separator();
                let Some(texture_id) = self.texture.as_ref().map(TextureHandle::id) else {
                    ui.centered_and_justified(|ui| ui.label(message));
                    return;
                };

                let available = ui.available_rect_before_wrap();
                let rotated = self.rotation_quarters % 2 == 1;
                let (image_width, image_height) = if rotated {
                    (self.image_size[1] as f32, self.image_size[0] as f32)
                } else {
                    (self.image_size[0] as f32, self.image_size[1] as f32)
                };
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
                let painter = ui.painter().with_clip_rect(image_rect);
                let mut mesh = egui::Mesh::with_texture(texture_id);
                let corners = [
                    (image_rect.left_top(), Pos2::new(0.0, 0.0)),
                    (image_rect.right_top(), Pos2::new(1.0, 0.0)),
                    (image_rect.right_bottom(), Pos2::new(1.0, 1.0)),
                    (image_rect.left_bottom(), Pos2::new(0.0, 1.0)),
                ];
                for (position, display_uv) in corners {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: position,
                        uv: source_uv(
                            display_uv,
                            self.rotation_quarters,
                            self.flip_horizontal,
                            self.flip_vertical,
                            crop,
                        ),
                        color: Color32::WHITE,
                    });
                }
                mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                painter.add(egui::Shape::mesh(mesh));
                if self.show_crosshair {
                    paint_crosshair(&painter, image_rect, CrosshairStyle::default());
                }
                ui.allocate_rect(image_rect, Sense::hover());
            });
        });

        context.request_repaint_after(Duration::from_millis(16));
    }
}

fn source_uv(
    mut display_uv: Pos2,
    rotation_quarters: u8,
    flip_horizontal: bool,
    flip_vertical: bool,
    crop: f32,
) -> Pos2 {
    if flip_horizontal {
        display_uv.x = 1.0 - display_uv.x;
    }
    if flip_vertical {
        display_uv.y = 1.0 - display_uv.y;
    }

    let source_uv = match rotation_quarters % 4 {
        0 => display_uv,
        1 => Pos2::new(display_uv.y, 1.0 - display_uv.x),
        2 => Pos2::new(1.0 - display_uv.x, 1.0 - display_uv.y),
        3 => Pos2::new(1.0 - display_uv.y, display_uv.x),
        _ => unreachable!(),
    };
    let visible_fraction = 1.0 - 2.0 * crop;
    Pos2::new(
        crop + source_uv.x * visible_fraction,
        crop + source_uv.y * visible_fraction,
    )
}

#[cfg(test)]
mod tests {
    use super::{frame_from_sample, source_uv};
    use eframe::egui::Pos2;
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

    #[test]
    fn maps_display_corners_for_each_quarter_turn() {
        let top_left = Pos2::new(0.0, 0.0);
        let expected_source_corners = [
            Pos2::new(0.0, 0.0),
            Pos2::new(0.0, 1.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(1.0, 0.0),
        ];
        for (rotation, expected) in expected_source_corners.into_iter().enumerate() {
            assert_eq!(
                source_uv(top_left, rotation as u8, false, false, 0.0),
                expected
            );
        }
    }

    #[test]
    fn maps_flips_and_zoom_crop_to_source_coordinates() {
        let top_left = Pos2::new(0.0, 0.0);
        assert_eq!(
            source_uv(top_left, 0, true, false, 0.0),
            Pos2::new(1.0, 0.0)
        );
        assert_eq!(
            source_uv(top_left, 0, false, true, 0.0),
            Pos2::new(0.0, 1.0)
        );
        assert_eq!(
            source_uv(top_left, 0, false, false, 0.25),
            Pos2::new(0.25, 0.25)
        );
    }
}
