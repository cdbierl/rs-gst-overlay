//! Draws the configurable crosshair overlay over the displayed video.

use egui::{Color32, Painter, Pos2, Rect, Stroke};

/// Visual settings for the centered crosshair.
#[derive(Clone, Copy, Debug)]
pub struct CrosshairStyle {
    /// Red, green, blue, and alpha channels in the range 0.0 to 1.0.
    pub color: [f32; 4],
    /// Stroke width in display points.
    pub line_width: f32,
    /// Fraction of the displayed video width and height covered by each line.
    pub length_fraction: f32,
    /// Radius of the centered circle in display points.
    pub circle_radius: f32,
}

impl Default for CrosshairStyle {
    /// Returns the default green crosshair appearance.
    fn default() -> Self {
        Self {
            color: [0.0, 1.0, 0.0, 0.85],
            line_width: 2.0,
            length_fraction: 0.85,
            circle_radius: 50.0,
        }
    }
}

/// Paints a centered crosshair within the supplied video rectangle.
///
/// Style values are clamped where necessary to keep color channels, line length,
/// and stroke dimensions within drawable ranges.
pub fn paint_crosshair(painter: &Painter, rect: Rect, style: CrosshairStyle) {
    let color = Color32::from_rgba_unmultiplied(
        (style.color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (style.color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (style.color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (style.color[3].clamp(0.0, 1.0) * 255.0) as u8,
    );
    let center = rect.center();
    let half_width = rect.width() * style.length_fraction.clamp(0.0, 1.0) / 2.0;
    let half_height = rect.height() * style.length_fraction.clamp(0.0, 1.0) / 2.0;
    let stroke = Stroke::new(style.line_width.max(0.1), color);

    painter.line_segment(
        [
            Pos2::new(center.x - half_width, center.y),
            Pos2::new(center.x + half_width, center.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(center.x, center.y - half_height),
            Pos2::new(center.x, center.y + half_height),
        ],
        stroke,
    );
    painter.circle_stroke(center, style.circle_radius.max(0.1), stroke);
}
