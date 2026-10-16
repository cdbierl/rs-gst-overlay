//! Implements the egui video player window, toolbar, and playback status bar.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use egui_material3::material_symbol::material_symbol_text;
use egui_material3::{MaterialIconButton, MaterialSlider};

use crate::overlay::{CrosshairStyle, RuleOfThirdsStyle, paint_crosshair, paint_rule_of_thirds};
use crate::pipeline_catalog::{PipelineCatalog, PipelineEntry};
use crate::stream::StreamHandle;

/// Largest zoom factor available from the toolbar or mouse wheel.
const MAX_ZOOM: f32 = 10.0;
/// Smallest zoom factor, which displays the full frame.
const MIN_ZOOM: f32 = 1.0;
/// Width of the toolbar zoom slider in display points.
const ZOOM_SLIDER_WIDTH: f32 = 100.0;
/// Exponential wheel delta multiplier used for zoom changes.
const ZOOM_SCROLL_SENSITIVITY: f32 = 0.002;
/// Minimum height reserved for the toolbar panel.
const MIN_TOOLBAR_HEIGHT: f32 = 48.0;
/// Height used to measure a single wrapped toolbar row.
const TOOLBAR_ROW_HEIGHT: f32 = 48.0;
/// Height reserved for the bottom status panel.
const STATUS_BAR_HEIGHT: f32 = 28.0;
/// Interval used to refresh the displayed frame rate.
const FPS_UPDATE_INTERVAL: Duration = Duration::from_secs(1);
/// Maximum delay between UI repaint requests.
const REPAINT_INTERVAL: Duration = Duration::from_millis(16);

/// Starts the stream worker and opens the native player window.
pub fn play(description: Option<String>) -> Result<(), String> {
    let (stream, worker) = crate::stream::start(description.clone())?;
    let app_stream = stream.clone();
    let mut pipeline_catalog = PipelineCatalog::load();
    let selected_pipeline =
        description.map(|description| pipeline_catalog.ensure_startup_preset(&description));
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
            Ok(Box::new(VideoApp::new(
                app_stream,
                pipeline_catalog,
                selected_pipeline,
            )))
        }),
    );

    drop(worker);
    result.map_err(|error| format!("could not run egui window: {error}"))
}

/// Holds the video texture and user-controlled display settings for the window.
struct VideoApp {
    stream: StreamHandle,
    texture: Option<TextureHandle>,
    image_size: [usize; 2],
    fps_interval_start: Instant,
    frames_in_interval: u64,
    frames_per_second: f32,
    zoom: f32,
    overlay_selection: OverlaySelection,
    rotation_quarters: u8,
    flip_horizontal: bool,
    flip_vertical: bool,
    toolbar_panel_height: f32,
    pipeline_catalog: PipelineCatalog,
    selected_pipeline: Option<PipelineEntry>,
    pipeline_search: String,
    preset_editor_open: bool,
    preset_editor_original_name: Option<String>,
    preset_editor_original_pipeline: Option<String>,
    preset_editor_name: String,
    preset_editor_pipeline: String,
    preset_editor_error: Option<String>,
    delete_preset_confirmation_open: bool,
    delete_preset_target: Option<PipelineEntry>,
    delete_preset_error: Option<String>,
}

/// Overlay drawn over the displayed video.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OverlaySelection {
    /// Draw no overlay.
    #[default]
    None,
    /// Draw the centered target-reticle crosshair.
    TargetReticle,
    /// Draw a rule-of-thirds framing grid.
    RuleOfThirds,
}

impl OverlaySelection {
    /// Returns the label shown in the overlay picker.
    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::TargetReticle => "Target Reticle",
            Self::RuleOfThirds => "Rule of Thirds",
        }
    }
}

impl VideoApp {
    /// Creates a player UI with the stream, pipeline catalog, and selected entry.
    fn new(
        stream: StreamHandle,
        pipeline_catalog: PipelineCatalog,
        selected_pipeline: Option<PipelineEntry>,
    ) -> Self {
        Self {
            stream,
            texture: None,
            image_size: [0, 0],
            fps_interval_start: Instant::now(),
            frames_in_interval: 0,
            frames_per_second: 0.0,
            zoom: MIN_ZOOM,
            overlay_selection: OverlaySelection::default(),
            rotation_quarters: 0,
            flip_horizontal: false,
            flip_vertical: false,
            toolbar_panel_height: MIN_TOOLBAR_HEIGHT,
            pipeline_catalog,
            selected_pipeline,
            pipeline_search: String::new(),
            preset_editor_open: false,
            preset_editor_original_name: None,
            preset_editor_original_pipeline: None,
            preset_editor_name: String::new(),
            preset_editor_pipeline: String::new(),
            preset_editor_error: None,
            delete_preset_confirmation_open: false,
            delete_preset_target: None,
            delete_preset_error: None,
        }
    }

    /// Uploads the newest stream frame to the egui texture, if one is available.
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

    /// Updates the displayed frame rate over a one-second measurement interval.
    fn update_fps(&mut self, received_frame: bool) {
        if received_frame {
            self.frames_in_interval += 1;
        }

        let now = Instant::now();
        let elapsed = now.duration_since(self.fps_interval_start);
        if elapsed >= FPS_UPDATE_INTERVAL {
            self.frames_per_second = self.frames_in_interval as f32 / elapsed.as_secs_f32();
            self.frames_in_interval = 0;
            self.fps_interval_start = now;
        }
    }

    /// Draws the playback controls and returns the measured height of their rows.
    fn show_toolbar(&mut self, ui: &mut egui::Ui, playing: bool) -> f32 {
        // Set the row height before adding labels so they align with the taller controls.
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().interact_size.y = TOOLBAR_ROW_HEIGHT;

        ui.with_layout(
            egui::Layout::top_down(egui::Align::Min).with_main_align(egui::Align::Center),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(12.0, 4.0);
                    self.show_pipeline_picker(ui);
                    self.show_overlay_picker(ui);
                    let zoom_value_width = body_text_width(ui, &format!("{:.2}", MAX_ZOOM));
                    let zoom_group_width = wrapped_items_width(
                        ui,
                        &[
                            body_text_width(ui, "Zoom"),
                            ZOOM_SLIDER_WIDTH,
                            zoom_value_width,
                        ],
                    );
                    show_wrapped_toolbar_group(ui, zoom_group_width, |ui| {
                        ui.label("Zoom");
                        ui.add(
                            MaterialSlider::new(&mut self.zoom, MIN_ZOOM..=MAX_ZOOM)
                                .width(ZOOM_SLIDER_WIDTH)
                                .show_value(false),
                        );
                        ui.label(format!("{:.2}", self.zoom));
                    });

                    let icon_button_size = 40.0;
                    let icon_controls_width = wrapped_items_width(ui, &[icon_button_size; 4]);
                    show_wrapped_toolbar_group(ui, icon_controls_width, |ui| {
                        if ui
                            .add(
                                MaterialIconButton::standard(material_symbol_text(
                                    "rotate_90_degrees_cw",
                                ))
                                .size(icon_button_size),
                            )
                            .on_hover_text("Rotate video 90° clockwise")
                            .clicked()
                        {
                            self.rotation_quarters = (self.rotation_quarters + 1) % 4;
                        }
                        ui.add(
                            MaterialIconButton::toggle(
                                material_symbol_text("swap_horiz"),
                                &mut self.flip_horizontal,
                            )
                            .size(icon_button_size),
                        )
                        .on_hover_text("Flip video horizontally");
                        ui.add(
                            MaterialIconButton::toggle(
                                material_symbol_text("swap_vert"),
                                &mut self.flip_vertical,
                            )
                            .size(icon_button_size),
                        )
                        .on_hover_text("Flip video vertically");

                        let (icon, tooltip) = if playing {
                            ("stop", "Stop pipeline")
                        } else {
                            ("play_arrow", "Start or restart pipeline")
                        };
                        if ui
                            .add_enabled(
                                playing || self.selected_pipeline.is_some(),
                                MaterialIconButton::filled(material_symbol_text(icon))
                                    .size(icon_button_size),
                            )
                            .on_hover_text(tooltip)
                            .clicked()
                            && !self.stream.set_playing(!playing)
                        {
                            log::error!("Pipeline controls are unavailable");
                        }
                    });
                })
                .response
                .rect
                .height()
            },
        )
        .inner
    }

    /// Shows the available video overlays beside the pipeline picker.
    fn show_overlay_picker(&mut self, ui: &mut egui::Ui) {
        let popup_item_height = ui.ctx().style().spacing.interact_size.y;
        let group_width = wrapped_items_width(ui, &[body_text_width(ui, "Overlay"), 140.0]);
        show_wrapped_toolbar_group(ui, group_width, |ui| {
            ui.label("Overlay");
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 32.0;
                egui::ComboBox::from_id_salt("overlay_picker")
                    .selected_text(self.overlay_selection.label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        ui.spacing_mut().interact_size.y = popup_item_height;
                        ui.selectable_value(
                            &mut self.overlay_selection,
                            OverlaySelection::None,
                            OverlaySelection::None.label(),
                        );
                        ui.selectable_value(
                            &mut self.overlay_selection,
                            OverlaySelection::TargetReticle,
                            OverlaySelection::TargetReticle.label(),
                        );
                        ui.selectable_value(
                            &mut self.overlay_selection,
                            OverlaySelection::RuleOfThirds,
                            OverlaySelection::RuleOfThirds.label(),
                        );
                    });
            });
        });
    }

    /// Shows searchable named presets and opens the selected pipeline.
    fn show_pipeline_picker(&mut self, ui: &mut egui::Ui) {
        let popup_item_height = ui.ctx().style().spacing.interact_size.y;
        let presets = self.pipeline_catalog.presets().to_vec();
        let selected_pipeline = self
            .selected_pipeline
            .as_ref()
            .map(|entry| entry.pipeline.as_str())
            .unwrap_or_default();
        let selected_name = self
            .selected_pipeline
            .as_ref()
            .map(|entry| entry.name.as_str())
            .unwrap_or("Select a pipeline");
        let can_manage_selected =
            self.pipeline_catalog.can_persist_changes() && self.selected_preset().is_some();
        let mut selected_entry = None;
        let mut edit_selected = false;
        let mut delete_selected = false;

        let group_width = wrapped_items_width(ui, &[body_text_width(ui, "Pipeline"), 200.0]);
        show_wrapped_toolbar_group(ui, group_width, |ui| {
            ui.label("Pipeline");
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 32.0;
                egui::ComboBox::from_id_salt("pipeline_picker")
                    .selected_text(selected_name)
                    .truncate()
                    .width(200.0)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show_ui(ui, |ui| {
                        ui.spacing_mut().interact_size.y = popup_item_height;
                        ui.add(
                            egui::TextEdit::singleline(&mut self.pipeline_search)
                                .hint_text("Search pipelines"),
                        );
                        ui.separator();

                        let query = self.pipeline_search.trim().to_owned();
                        let mut found_match = false;
                        egui::ScrollArea::vertical()
                            .max_height(250.0)
                            .show(ui, |ui| {
                                for entry in &presets {
                                    if !matches_pipeline_query(entry, &query) {
                                        continue;
                                    }
                                    found_match = true;
                                    let response = ui
                                        .selectable_label(
                                            selected_pipeline == entry.pipeline
                                                && selected_name == entry.name,
                                            &entry.name,
                                        )
                                        .on_hover_text(&entry.pipeline);
                                    if response.clicked() {
                                        selected_entry = Some(entry.clone());
                                        ui.close();
                                    }
                                }

                                if !found_match {
                                    ui.label("No matching pipelines");
                                }

                                ui.separator();
                                if ui
                                    .add_enabled(
                                        can_manage_selected,
                                        egui::Button::new("Edit selected preset…"),
                                    )
                                    .clicked()
                                {
                                    edit_selected = true;
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        can_manage_selected,
                                        egui::Button::new("Delete selected preset…"),
                                    )
                                    .clicked()
                                {
                                    delete_selected = true;
                                    ui.close();
                                }
                            });
                    });
            });
        });

        if let Some(entry) = selected_entry {
            self.select_pipeline(entry);
        }
        if edit_selected {
            self.open_preset_editor();
        }
        if delete_selected {
            self.open_delete_preset_confirmation();
        }
    }

    /// Finds the exact catalog entry currently selected by the player.
    fn selected_preset(&self) -> Option<&PipelineEntry> {
        let selected = self.selected_pipeline.as_ref()?;
        self.pipeline_catalog
            .presets()
            .iter()
            .find(|entry| entry.name == selected.name && entry.pipeline == selected.pipeline)
    }

    /// Switches the active pipeline to the selected preset.
    fn select_pipeline(&mut self, entry: PipelineEntry) {
        self.pipeline_search.clear();
        if (self
            .selected_pipeline
            .as_ref()
            .is_none_or(|selected| selected.pipeline != entry.pipeline)
            || !self.stream.status().playing)
            && !self.stream.load_pipeline(entry.pipeline.clone())
        {
            log::error!("Pipeline controls are unavailable");
            return;
        }

        log::info!("Selected pipeline preset '{}'", entry.name);
        self.selected_pipeline = Some(entry);
    }

    /// Copies the active named preset into the editor dialog.
    fn open_preset_editor(&mut self) {
        let Some(preset) = self.selected_preset().cloned() else {
            return;
        };

        self.preset_editor_original_name = Some(preset.name.clone());
        self.preset_editor_original_pipeline = Some(preset.pipeline.clone());
        self.preset_editor_name = preset.name;
        self.preset_editor_pipeline = preset.pipeline;
        self.preset_editor_error = None;
        self.preset_editor_open = true;
    }

    /// Draws a dialog for renaming a preset or editing its full GStreamer string.
    fn show_preset_editor(&mut self, context: &egui::Context) {
        if !self.preset_editor_open {
            return;
        }

        let mut open = self.preset_editor_open;
        let mut save_requested = false;
        let mut cancel_requested = false;
        egui::Window::new("Edit pipeline preset")
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .show(context, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.preset_editor_name);
                ui.add_space(6.0);
                ui.label("GStreamer pipeline");
                ui.add(
                    egui::TextEdit::multiline(&mut self.preset_editor_pipeline)
                        .desired_width(540.0)
                        .desired_rows(6)
                        .code_editor(),
                );

                if let Some(error) = &self.preset_editor_error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }

                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save_requested = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_requested = true;
                    }
                });
            });
        self.preset_editor_open = open && !cancel_requested;

        if !self.preset_editor_open && !save_requested {
            self.preset_editor_original_name = None;
            self.preset_editor_original_pipeline = None;
            self.preset_editor_error = None;
        }

        if save_requested {
            self.save_preset_editor();
        }
    }

    /// Persists the edited preset and reloads its pipeline if the string changed.
    fn save_preset_editor(&mut self) {
        let Some(original_name) = self.preset_editor_original_name.clone() else {
            return;
        };
        let Some(original_pipeline) = self.preset_editor_original_pipeline.clone() else {
            return;
        };
        match self.pipeline_catalog.update_preset(
            &original_name,
            &original_pipeline,
            &self.preset_editor_name,
            &self.preset_editor_pipeline,
        ) {
            Ok(entry) => {
                if entry.pipeline != original_pipeline
                    && !self.stream.load_pipeline(entry.pipeline.clone())
                {
                    log::error!("Pipeline controls are unavailable");
                }
                self.selected_pipeline = Some(entry);
                self.pipeline_search.clear();
                self.preset_editor_error = None;
                self.preset_editor_open = false;
                self.preset_editor_original_name = None;
                self.preset_editor_original_pipeline = None;
            }
            Err(error) => self.preset_editor_error = Some(error),
        }
    }

    /// Opens a confirmation dialog for the active named preset.
    fn open_delete_preset_confirmation(&mut self) {
        let Some(preset) = self.selected_preset().cloned() else {
            return;
        };

        self.delete_preset_target = Some(preset);
        self.delete_preset_error = None;
        self.delete_preset_confirmation_open = true;
    }

    /// Confirms and applies deletion of the selected preset.
    fn show_delete_preset_confirmation(&mut self, context: &egui::Context) {
        if !self.delete_preset_confirmation_open {
            return;
        }

        let mut open = self.delete_preset_confirmation_open;
        let mut confirm_delete = false;
        let mut cancel_delete = false;
        egui::Window::new("Delete pipeline preset")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(context, |ui| {
                if let Some(target) = &self.delete_preset_target {
                    ui.label(format!("Delete '{}' from pipelines.yaml?", target.name));
                    ui.label("Deleting a preset does not stop the current pipeline.");
                }
                if let Some(error) = &self.delete_preset_error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }

                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        confirm_delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_delete = true;
                    }
                });
            });
        self.delete_preset_confirmation_open = open && !cancel_delete;

        if !self.delete_preset_confirmation_open && !confirm_delete {
            self.delete_preset_target = None;
            self.delete_preset_error = None;
        }

        if confirm_delete {
            self.delete_selected_preset();
        }
    }

    /// Removes the confirmed preset while keeping its current runtime pipeline active.
    fn delete_selected_preset(&mut self) {
        let Some(target) = self.delete_preset_target.take() else {
            return;
        };
        match self
            .pipeline_catalog
            .delete_preset(&target.name, &target.pipeline)
        {
            Ok(()) => {
                self.selected_pipeline = Some(
                    self.pipeline_catalog
                        .find_preset(&target.pipeline)
                        .unwrap_or_else(|| PipelineEntry {
                            name: "Current pipeline".to_owned(),
                            pipeline: target.pipeline.clone(),
                        }),
                );
                self.delete_preset_confirmation_open = false;
                self.delete_preset_error = None;
            }
            Err(error) => {
                self.delete_preset_target = Some(target);
                self.delete_preset_error = Some(error);
            }
        }
    }

    /// Draws the frame rate and current pipeline status in the bottom panel.
    fn show_status_bar(&self, ui: &mut egui::Ui, message: &str) {
        ui.horizontal_centered(|ui| {
            ui.label(format!("FPS: {:.1}", self.frames_per_second));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(message).truncate());
            });
        });
    }

    /// Fits the current frame into the central panel and applies view transforms.
    fn show_video(&mut self, ui: &mut egui::Ui) {
        let Some(texture_id) = self.texture.as_ref().map(TextureHandle::id) else {
            let message = if self.selected_pipeline.is_some() {
                "Waiting for video frames…"
            } else {
                "Select a pipeline to start playback"
            };
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

        if image_rect.contains(ui.input(|input| input.pointer.hover_pos().unwrap_or(Pos2::ZERO))) {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.zoom = (self.zoom * (scroll * ZOOM_SCROLL_SENSITIVITY).exp())
                    .clamp(MIN_ZOOM, MAX_ZOOM);
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
        match self.overlay_selection {
            OverlaySelection::None => {}
            OverlaySelection::TargetReticle => {
                paint_crosshair(&painter, image_rect, CrosshairStyle::default());
            }
            OverlaySelection::RuleOfThirds => {
                paint_rule_of_thirds(&painter, image_rect, RuleOfThirdsStyle::default())
            }
        }
        ui.allocate_rect(image_rect, Sense::hover());
    }
}

impl eframe::App for VideoApp {
    /// Updates stream metrics and lays out the toolbar, video, and status panels.
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let received_frame = self.update_texture(context);
        self.update_fps(received_frame);
        let status = self.stream.status();

        let toolbar_frame = egui::Frame::side_top_panel(&context.style());
        // The panel needs its height before its contents are laid out, so apply the previous
        // frame's measurement and use this frame's measurement on the next pass.
        let toolbar_content_height = egui::TopBottomPanel::top("toolbar")
            .frame(toolbar_frame)
            .exact_height(self.toolbar_panel_height)
            .show(context, |ui| self.show_toolbar(ui, status.playing))
            .inner;
        self.toolbar_panel_height =
            (toolbar_content_height + toolbar_frame.inner_margin.sum().y).max(MIN_TOOLBAR_HEIGHT);

        egui::TopBottomPanel::bottom("player_status")
            .exact_height(STATUS_BAR_HEIGHT)
            .show(context, |ui| self.show_status_bar(ui, &status.message));

        egui::CentralPanel::default().show(context, |ui| self.show_video(ui));
        self.show_preset_editor(context);
        self.show_delete_preset_confirmation(context);

        context.request_repaint_after(REPAINT_INTERVAL);
    }
}

/// Matches a pipeline entry against a case-insensitive name or description query.
fn matches_pipeline_query(entry: &PipelineEntry, query: &str) -> bool {
    let query = query.to_lowercase();
    query.is_empty()
        || entry.name.to_lowercase().contains(&query)
        || entry.pipeline.to_lowercase().contains(&query)
}

/// Allocates a measured toolbar group so the wrapping row can move it intact.
fn show_wrapped_toolbar_group<R>(
    ui: &mut egui::Ui,
    width: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        Vec2::new(width, TOOLBAR_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        add_contents,
    )
    .inner
}

/// Measures a row of items, including the spacing between them.
fn wrapped_items_width(ui: &egui::Ui, item_widths: &[f32]) -> f32 {
    item_widths.iter().sum::<f32>()
        + ui.spacing().item_spacing.x * item_widths.len().saturating_sub(1) as f32
}

/// Measures a single line of body text for sizing a wrapped toolbar group.
fn body_text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        )
        .size()
        .x
}

/// Maps a display-space texture coordinate through flips, rotation, and zoom crop.
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
/// Tests for mapping displayed image corners to source texture coordinates.
mod tests {
    use super::{OverlaySelection, matches_pipeline_query, source_uv};
    use crate::pipeline_catalog::PipelineEntry;
    use eframe::egui::Pos2;

    #[test]
    /// Maps the displayed upper-left corner correctly for each quarter turn.
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
    /// Applies horizontal and vertical flips and a centered zoom crop.
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

    #[test]
    /// Searches pipeline choices by either their name or launch description.
    fn pipeline_search_matches_names_and_descriptions_case_insensitively() {
        let entry = PipelineEntry {
            name: "RTSP Camera".to_owned(),
            pipeline: "rtspsrc location=rtsp://camera/stream".to_owned(),
        };

        assert!(matches_pipeline_query(&entry, "camera"));
        assert!(matches_pipeline_query(&entry, "RTSP://CAMERA"));
        assert!(!matches_pipeline_query(&entry, "file source"));
    }

    #[test]
    /// Starts with no overlay selected.
    fn overlay_selection_defaults_to_none() {
        assert_eq!(OverlaySelection::default(), OverlaySelection::None);
    }
}
