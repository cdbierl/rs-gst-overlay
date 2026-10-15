use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use egui_material3::material_symbol::material_symbol_text;
use egui_material3::{MaterialIconButton, MaterialSlider, MaterialSwitch};

use crate::overlay::{CrosshairStyle, paint_crosshair};
use crate::stream::StreamHandle;

const MAX_ZOOM: f32 = 10.0;

pub fn play(description: String) -> Result<(), String> {
    let (stream, worker) = crate::stream::start(description)?;
    let app_stream = stream.clone();
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
            Ok(Box::new(VideoApp::new(app_stream)))
        }),
    );

    drop(worker);
    result.map_err(|error| format!("could not run egui window: {error}"))
}

struct VideoApp {
    stream: StreamHandle,
    texture: Option<TextureHandle>,
    image_size: [usize; 2],
    fps_interval_start: Instant,
    frames_in_interval: u64,
    frames_per_second: f32,
    zoom: f32,
    show_crosshair: bool,
    rotation_quarters: u8,
    flip_horizontal: bool,
    flip_vertical: bool,
}

impl VideoApp {
    fn new(stream: StreamHandle) -> Self {
        Self {
            stream,
            texture: None,
            image_size: [0, 0],
            fps_interval_start: Instant::now(),
            frames_in_interval: 0,
            frames_per_second: 0.0,
            zoom: 1.0,
            show_crosshair: true,
            rotation_quarters: 0,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }

    fn update_texture(&mut self, context: &egui::Context) -> bool {
        let frame = self.stream.take_latest_frame();
        let Some(frame) = frame else {
            return false;
        };

        let image =
            egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        if self.image_size != [frame.width, frame.height] {
            self.image_size = [frame.width, frame.height];
            self.texture = Some(context.load_texture("video-frame", image, TextureOptions::LINEAR));
        } else if let Some(texture) = self.texture.as_mut() {
            texture.set(image, TextureOptions::LINEAR);
        }

        true
    }

    fn update_fps(&mut self, received_frame: bool) {
        if received_frame {
            self.frames_in_interval += 1;
        }

        let now = Instant::now();
        let elapsed = now.duration_since(self.fps_interval_start);
        if elapsed >= Duration::from_secs(1) {
            self.frames_per_second = self.frames_in_interval as f32 / elapsed.as_secs_f32();
            self.frames_in_interval = 0;
            self.fps_interval_start = now;
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui, playing: bool) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.label("Crosshair");
            ui.add(MaterialSwitch::new(&mut self.show_crosshair));
            ui.label("Zoom");
            ui.add(MaterialSlider::new(&mut self.zoom, 1.0..=MAX_ZOOM).width(100.0));

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

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, tooltip) = if playing {
                    ("stop", "Stop pipeline")
                } else {
                    ("play_arrow", "Start or restart pipeline")
                };
                if ui
                    .add(MaterialIconButton::filled(material_symbol_text(icon)))
                    .on_hover_text(tooltip)
                    .clicked()
                    && !self.stream.set_playing(!playing)
                {
                    log::error!("Pipeline controls are unavailable");
                }
            });
        });
    }

    fn show_status_bar(&self, ui: &mut egui::Ui, message: &str) {
        ui.horizontal_centered(|ui| {
            ui.label(format!("FPS: {:.1}", self.frames_per_second));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(message).truncate());
            });
        });
    }

    fn show_video(&mut self, ui: &mut egui::Ui) {
        let Some(texture_id) = self.texture.as_ref().map(TextureHandle::id) else {
            ui.centered_and_justified(|ui| ui.label("Waiting for video frames…"));
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

        if image_rect.contains(ui.input(|input| input.pointer.hover_pos().unwrap_or(Pos2::ZERO))) {
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
    }
}

impl eframe::App for VideoApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let received_frame = self.update_texture(context);
        self.update_fps(received_frame);
        let status = self.stream.status();

        egui::TopBottomPanel::top("toolbar")
            .exact_height(48.0)
            .show(context, |ui| self.show_toolbar(ui, status.playing));

        egui::TopBottomPanel::bottom("player_status")
            .exact_height(30.0)
            .show(context, |ui| self.show_status_bar(ui, &status.message));

        egui::CentralPanel::default().show(context, |ui| self.show_video(ui));

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
    use super::source_uv;
    use eframe::egui::Pos2;

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
