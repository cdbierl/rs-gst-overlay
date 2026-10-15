use gst::prelude::*;
use gstreamer as gst;

pub fn parse(description: &str) -> Result<gst::Pipeline, String> {
    if description.trim().is_empty() {
        return Err("the pipeline is empty".to_owned());
    }

    gst::parse::launch(description)
        .map_err(|error| format!("could not parse pipeline: {error}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "the pipeline description did not produce a GStreamer pipeline".to_owned())
}
