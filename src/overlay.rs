//! Applies pixel filters and draws overlays over the displayed video.

use egui::{Color32, Painter, Pos2, Rect, Stroke};

/// Pixel transformation applied before a video frame is uploaded as a texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextureFilter {
    /// Preserve the source colors.
    Color,
    /// Convert RGB channels to luminance.
    Grayscale,
    /// Invert RGB channels.
    Negative,
}

/// Applies a pixel filter to RGBA data without changing alpha channels.
pub(crate) fn apply_texture_filter(rgba: &mut [u8], filter: TextureFilter) {
    match filter {
        TextureFilter::Color => {}
        TextureFilter::Grayscale => {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                let luminance = ((u16::from(pixel[0]) * 77
                    + u16::from(pixel[1]) * 150
                    + u16::from(pixel[2]) * 29
                    + 128)
                    >> 8) as u8;
                pixel[..3].fill(luminance);
            }
        }
        TextureFilter::Negative => {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = 255 - *channel;
                }
            }
        }
    }
}

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

/// Visual settings for the rule-of-thirds grid.
#[derive(Clone, Copy, Debug)]
pub struct RuleOfThirdsStyle {
    /// Red, green, blue, and alpha channels in the range 0.0 to 1.0.
    pub color: [f32; 4],
    /// Stroke width in display points.
    pub line_width: f32,
}

impl Default for RuleOfThirdsStyle {
    /// Returns the default green grid appearance.
    fn default() -> Self {
        Self {
            color: [0.0, 1.0, 0.0, 0.85],
            line_width: 2.0,
        }
    }
}

/// Converts normalized RGBA channels to an egui color.
fn overlay_color(color: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Paints a centered crosshair within the supplied video rectangle.
///
/// Style values are clamped where necessary to keep color channels, line length,
/// and stroke dimensions within drawable ranges.
pub fn paint_crosshair(painter: &Painter, rect: Rect, style: CrosshairStyle) {
    let color = overlay_color(style.color);
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

/// Paints a rule-of-thirds grid within the supplied video rectangle.
pub fn paint_rule_of_thirds(painter: &Painter, rect: Rect, style: RuleOfThirdsStyle) {
    let stroke = Stroke::new(style.line_width.max(0.1), overlay_color(style.color));
    let third_width = rect.width() / 3.0;
    let third_height = rect.height() / 3.0;

    for division in [1.0, 2.0] {
        let x = rect.left() + third_width * division;
        let y = rect.top() + third_height * division;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            stroke,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{RuleOfThirdsStyle, TextureFilter, apply_texture_filter, paint_rule_of_thirds};
    use egui::{Color32, Id, LayerId, Order, Pos2, RawInput, Rect, Shape, Stroke, Vec2};

    #[test]
    fn rule_of_thirds_paints_four_guides_with_the_configured_style() {
        let context = egui::Context::default();
        let rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(310.0, 220.0));
        let style = RuleOfThirdsStyle {
            color: [0.25, 0.5, 0.75, 0.8],
            line_width: 2.5,
        };
        let output = context.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                ..Default::default()
            },
            |context| {
                let painter =
                    context.layer_painter(LayerId::new(Order::Foreground, Id::new("test")));
                paint_rule_of_thirds(&painter, rect, style);
            },
        );
        let lines = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::LineSegment { points, stroke } => Some((*points, *stroke)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let color = Color32::from_rgba_unmultiplied(63, 127, 191, 204);
        let expected_lines = [
            [Pos2::new(110.0, 20.0), Pos2::new(110.0, 220.0)],
            [
                Pos2::new(10.0, 20.0 + 200.0 / 3.0),
                Pos2::new(310.0, 20.0 + 200.0 / 3.0),
            ],
            [Pos2::new(210.0, 20.0), Pos2::new(210.0, 220.0)],
            [
                Pos2::new(10.0, 20.0 + 400.0 / 3.0),
                Pos2::new(310.0, 20.0 + 400.0 / 3.0),
            ],
        ];

        assert_eq!(lines.len(), expected_lines.len());
        for ((points, stroke), expected_points) in lines.into_iter().zip(expected_lines) {
            assert_eq!(points, expected_points);
            assert_eq!(stroke, Stroke::new(2.5_f32, color));
        }
    }

    #[test]
    fn grayscale_filter_preserves_alpha() {
        let mut rgba = vec![255, 0, 0, 17, 0, 255, 0, 128, 0, 0, 255, 255];

        apply_texture_filter(&mut rgba, TextureFilter::Grayscale);

        assert_eq!(
            rgba,
            vec![77, 77, 77, 17, 149, 149, 149, 128, 29, 29, 29, 255]
        );
    }

    #[test]
    fn negative_filter_preserves_alpha() {
        let mut rgba = vec![0, 127, 255, 17, 20, 100, 200, 128];

        apply_texture_filter(&mut rgba, TextureFilter::Negative);

        assert_eq!(rgba, vec![255, 128, 0, 17, 235, 155, 55, 128]);
    }
}
