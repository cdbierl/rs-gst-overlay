pub const USAGE: &str = "Usage: rs_gst_overlay [--no-overlay] [--pipeline|-p] <PIPELINE>
       rs_gst_overlay [--no-overlay] '<PIPELINE>'

Play a GStreamer pipeline in an egui window. The pipeline must contain an
`appsink` named `video_sink`; its frames are displayed in the window. By
default, the window draws a centered crosshair over the video. Use
--no-overlay to hide the crosshair.

The pipeline may be supplied with --pipeline (or -p), as a quoted positional
argument, or through GST_PIPELINE. A command-line pipeline takes precedence.
A .env file in the current directory or a parent directory is loaded at startup.
Logging follows RUST_LOG and defaults to info (for example, RUST_LOG=debug).

Examples:
  rs_gst_overlay --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
  rs_gst_overlay --no-overlay --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
  GST_PIPELINE='videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink' rs_gst_overlay";

pub enum Action {
    Help,
    Run {
        pipeline: Option<String>,
        no_overlay: bool,
    },
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Action, String> {
    let mut args = args.into_iter();
    let mut pipeline = None;
    let mut no_overlay = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "--no-overlay" => no_overlay = true,
            "-p" | "--pipeline" => {
                if pipeline.is_some() {
                    return Err("provide the pipeline only once".to_owned());
                }
                pipeline = Some(
                    args.next()
                        .ok_or_else(|| format!("{arg} requires a pipeline string"))?,
                );
            }
            _ if arg.starts_with("--pipeline=") => {
                if pipeline.is_some() {
                    return Err("provide the pipeline only once".to_owned());
                }
                pipeline = Some(arg["--pipeline=".len()..].to_owned());
            }
            _ if !arg.starts_with('-') => {
                if pipeline.is_some() {
                    return Err("provide the pipeline only once".to_owned());
                }
                pipeline = Some(arg);
            }
            _ => return Err(format!("unexpected argument: {arg}")),
        }
    }

    Ok(Action::Run {
        pipeline,
        no_overlay,
    })
}

#[cfg(test)]
mod tests {
    use super::{Action, parse_args};

    #[test]
    fn accepts_option_and_positional_pipeline() {
        assert!(matches!(
            parse_args(["--pipeline".to_owned(), "videotestsrc ! fakesink".to_owned()]),
            Ok(Action::Run {
                pipeline: Some(pipeline),
                no_overlay: false,
            }) if pipeline == "videotestsrc ! fakesink"
        ));
        assert!(matches!(
            parse_args(["videotestsrc ! fakesink".to_owned()]),
            Ok(Action::Run {
                pipeline: Some(pipeline),
                no_overlay: false,
            }) if pipeline == "videotestsrc ! fakesink"
        ));
    }

    #[test]
    fn accepts_no_overlay_with_pipeline_option_or_positional_pipeline() {
        assert!(matches!(
            parse_args([
                "--no-overlay".to_owned(),
                "--pipeline".to_owned(),
                "videotestsrc ! fakesink".to_owned()
            ]),
            Ok(Action::Run {
                pipeline: Some(pipeline),
                no_overlay: true,
            }) if pipeline == "videotestsrc ! fakesink"
        ));
        assert!(matches!(
            parse_args([
                "videotestsrc ! fakesink".to_owned(),
                "--no-overlay".to_owned()
            ]),
            Ok(Action::Run {
                pipeline: Some(pipeline),
                no_overlay: true,
            }) if pipeline == "videotestsrc ! fakesink"
        ));
    }

    #[test]
    fn rejects_duplicate_or_unknown_arguments() {
        assert!(
            parse_args([
                "--pipeline".to_owned(),
                "a ! b".to_owned(),
                "another pipeline".to_owned()
            ])
            .is_err()
        );
        assert!(parse_args(["--unknown".to_owned()]).is_err());
    }
}
