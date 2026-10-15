//! Parses command-line arguments and provides the application's usage text.

/// Command-line usage and examples printed by the help action.
pub const USAGE: &str = "Usage: rs_gst_overlay [--pipeline|-p] <PIPELINE>
       rs_gst_overlay '<PIPELINE>'

Play a GStreamer pipeline in an egui window. The pipeline must contain an
`appsink` named `video_sink`; its frames are displayed in the window. By
default, the window draws a centered crosshair over the video. Toggle it with
the Crosshair switch in the window.

The pipeline may be supplied with --pipeline (or -p), as --pipeline=<PIPELINE>,
as a quoted positional argument, or through GST_PIPELINE. A command-line
pipeline takes precedence.
A .env file in the current directory or a parent directory is loaded at startup.
Logging follows RUST_LOG and defaults to info (for example, RUST_LOG=debug).

Examples:
  rs_gst_overlay --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
  GST_PIPELINE='videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink' rs_gst_overlay";

/// The action selected from the command-line arguments.
pub enum Action {
    /// Print the usage text and exit successfully.
    Help,
    /// Run the player, optionally using a pipeline supplied on the command line.
    Run { pipeline: Option<String> },
}

/// Parses supported options and returns the requested action or an error message.
///
/// A pipeline may be provided positionally, with `--pipeline` / `-p`, or as
/// `--pipeline=<PIPELINE>`.
/// Duplicate pipelines and unknown options are rejected.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Action, String> {
    let mut args = args.into_iter();
    let mut pipeline = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
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

    Ok(Action::Run { pipeline })
}

#[cfg(test)]
/// Tests for accepted and rejected command-line forms.
mod tests {
    use super::{Action, parse_args};

    #[test]
    /// Accepts both the named pipeline option and a positional pipeline.
    fn accepts_option_and_positional_pipeline() {
        assert!(matches!(
            parse_args(["--pipeline".to_owned(), "videotestsrc ! fakesink".to_owned()]),
            Ok(Action::Run {
                pipeline: Some(pipeline),
            }) if pipeline == "videotestsrc ! fakesink"
        ));
        assert!(matches!(
            parse_args(["videotestsrc ! fakesink".to_owned()]),
            Ok(Action::Run {
                pipeline: Some(pipeline),
            }) if pipeline == "videotestsrc ! fakesink"
        ));
    }

    #[test]
    /// Rejects the removed option for disabling the crosshair overlay.
    fn rejects_removed_no_overlay_option() {
        assert!(parse_args(["--no-overlay".to_owned()]).is_err());
    }

    #[test]
    /// Rejects repeated pipeline arguments and unrecognized options.
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
