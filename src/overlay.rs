use gst::prelude::*;
use gstreamer as gst;

/// Visual settings for the centered crosshair.
#[derive(Clone, Copy, Debug)]
pub struct CrosshairStyle {
    /// Red, green, blue, and alpha channels in the range 0.0 to 1.0.
    pub color: [f64; 4],
    /// Stroke width in pixels.
    pub line_width: f64,
    /// Fraction of the video width and height covered by each line.
    pub length_fraction: f64,
}

impl Default for CrosshairStyle {
    fn default() -> Self {
        Self {
            color: [0.0, 1.0, 0.0, 0.85],
            line_width: 4.0,
            length_fraction: 0.85,
        }
    }
}

impl CrosshairStyle {
    fn validate(self) -> Result<Self, String> {
        if !self.line_width.is_finite() || self.line_width <= 0.0 {
            return Err("crosshair line width must be a positive finite number".to_owned());
        }
        if !self.length_fraction.is_finite() || !(0.0..=1.0).contains(&self.length_fraction) {
            return Err("crosshair length fraction must be between 0.0 and 1.0".to_owned());
        }
        if self
            .color
            .iter()
            .any(|channel| !channel.is_finite() || !(0.0..=1.0).contains(channel))
        {
            return Err("crosshair color channels must be between 0.0 and 1.0".to_owned());
        }
        Ok(self)
    }
}

/// Configure the pipeline's existing `cairooverlay` named `overlay`.
pub fn configure_crosshair(pipeline: &gst::Pipeline, style: CrosshairStyle) -> Result<(), String> {
    let style = style.validate()?;
    let overlay = pipeline
        .by_name("overlay")
        .ok_or_else(|| "pipeline must contain a cairooverlay named 'overlay'".to_owned())?;
    let factory_name = overlay
        .factory()
        .map(|factory| factory.name().to_string())
        .unwrap_or_default();
    if factory_name != "cairooverlay" {
        return Err(format!(
            "pipeline element 'overlay' must be cairooverlay, found '{factory_name}'"
        ));
    }

    connect_draw_handler(&overlay, style);
    log::info!("Configured crosshair on the pipeline's video sink");
    Ok(())
}

fn connect_draw_handler(overlay: &gst::Element, style: CrosshairStyle) {
    overlay.connect("draw", false, move |values| {
        if let Ok(context) = values[1].get::<cairo::Context>() {
            draw_crosshair(&context, style);
        }
        None
    });
}

fn draw_crosshair(context: &cairo::Context, style: CrosshairStyle) {
    let Ok((left, top, right, bottom)) = context.clip_extents() else {
        return;
    };

    let width = right - left;
    let height = bottom - top;
    let center_x = left + width / 2.0;
    let center_y = top + height / 2.0;

    context.set_source_rgba(
        style.color[0],
        style.color[1],
        style.color[2],
        style.color[3],
    );
    context.set_line_width(style.line_width);
    context.move_to(center_x - width * style.length_fraction / 2.0, center_y);
    context.line_to(center_x + width * style.length_fraction / 2.0, center_y);
    context.move_to(center_x, center_y - height * style.length_fraction / 2.0);
    context.line_to(center_x, center_y + height * style.length_fraction / 2.0);
    let _ = context.stroke();
}

#[cfg(test)]
mod tests {
    use super::{CrosshairStyle, configure_crosshair};
    use gst::prelude::*;
    use gstreamer as gst;

    #[test]
    fn accepts_default_style_and_rejects_invalid_dimensions() {
        assert!(CrosshairStyle::default().validate().is_ok());
        assert!(
            CrosshairStyle {
                line_width: 0.0,
                ..CrosshairStyle::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            CrosshairStyle {
                length_fraction: 1.1,
                ..CrosshairStyle::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn configures_the_named_overlay_without_adding_another() {
        gst::init().unwrap();
        if gst::ElementFactory::find("cairooverlay").is_none()
            || gst::ElementFactory::find("autovideosink").is_none()
        {
            return;
        }

        let pipeline = gst::parse::launch(
            "videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        configure_crosshair(&pipeline, CrosshairStyle::default()).unwrap();

        let overlay_count = pipeline
            .iterate_elements()
            .into_iter()
            .map(Result::unwrap)
            .filter(|element| {
                element
                    .factory()
                    .is_some_and(|factory| factory.name() == "cairooverlay")
            })
            .count();
        assert_eq!(overlay_count, 1);
    }

    #[test]
    fn draws_while_processing_a_finite_video_stream() {
        gst::init().unwrap();
        if gst::ElementFactory::find("cairooverlay").is_none()
            || gst::ElementFactory::find("videoconvert").is_none()
            || gst::ElementFactory::find("fakesink").is_none()
        {
            return;
        }

        let pipeline = gst::parse::launch(
            "videotestsrc num-buffers=3 ! videoconvert ! cairooverlay name=overlay ! fakesink",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        configure_crosshair(&pipeline, CrosshairStyle::default()).unwrap();
        let bus = pipeline.bus().unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();

        let reached_eos = loop {
            let Some(message) = bus.timed_pop(gst::ClockTime::from_seconds(5)) else {
                break false;
            };
            match message.view() {
                gst::MessageView::Eos(..) => break true,
                gst::MessageView::Error(error) => panic!("pipeline error: {}", error.error()),
                _ => {}
            }
        };

        pipeline.set_state(gst::State::Null).unwrap();
        assert!(reached_eos, "test video pipeline did not reach EOS");
    }
}
