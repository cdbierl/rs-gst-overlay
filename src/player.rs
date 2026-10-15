use gst::prelude::*;
use gstreamer as gst;

pub fn play(pipeline: gst::Pipeline) -> Result<(), String> {
    let bus = pipeline
        .bus()
        .ok_or_else(|| "the GStreamer pipeline has no bus".to_owned())?;

    log::info!("Starting GStreamer pipeline");
    if let Err(state_error) = pipeline.set_state(gst::State::Playing) {
        let playback_error = bus
            .pop_filtered(&[gst::MessageType::Error])
            .and_then(|message| message_error(&message))
            .unwrap_or_else(|| format!("could not start pipeline: {state_error}"));
        let stop_result = pipeline
            .set_state(gst::State::Null)
            .map_err(|error| format!("could not stop pipeline: {error}"));
        stop_result?;
        return Err(playback_error);
    }

    log::info!("Waiting for the pipeline to start");
    let playback_result = wait_for_eos_or_error(&bus, &pipeline);
    let stop_result = pipeline
        .set_state(gst::State::Null)
        .map_err(|error| format!("could not stop pipeline: {error}"));

    playback_result?;
    stop_result?;
    Ok(())
}

fn wait_for_eos_or_error(bus: &gst::Bus, pipeline: &gst::Pipeline) -> Result<(), String> {
    loop {
        let Some(message) = bus.timed_pop(gst::ClockTime::NONE) else {
            continue;
        };

        match message.view() {
            gst::MessageView::Eos(..) => {
                log::info!("Pipeline reached end of stream");
                return Ok(());
            }
            gst::MessageView::StateChanged(state) if is_pipeline_message(&message, pipeline) => {
                log::info!(
                    "Pipeline state: {:?} -> {:?} (pending: {:?})",
                    state.old(),
                    state.current(),
                    state.pending()
                );
            }
            gst::MessageView::AsyncDone(..) if is_pipeline_message(&message, pipeline) => {
                log::info!("Pipeline preroll complete");
            }
            gst::MessageView::Buffering(buffering) => {
                log::info!("Pipeline buffering: {}%", buffering.percent());
            }
            gst::MessageView::Error(..) => {
                return Err(message_error(&message).unwrap_or_else(|| "pipeline error".to_owned()));
            }
            gst::MessageView::Warning(warning) => {
                log::warn!(
                    "GStreamer warning: {} ({:?})",
                    warning.error(),
                    warning.debug()
                );
            }
            _ => {}
        }
    }
}

fn is_pipeline_message(message: &gst::Message, pipeline: &gst::Pipeline) -> bool {
    message
        .src()
        .is_some_and(|source| source == pipeline.upcast_ref::<gst::Object>())
}

fn message_error(message: &gst::Message) -> Option<String> {
    match message.view() {
        gst::MessageView::Error(error) => Some(format!(
            "pipeline error: {} ({:?})",
            error.error(),
            error.debug()
        )),
        _ => None,
    }
}
