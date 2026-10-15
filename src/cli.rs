pub const USAGE: &str = "Usage: rs_gst [--pipeline|-p] <PIPELINE>
       rs_gst '<PIPELINE>'

Play a GStreamer pipeline using the Rust GStreamer bindings.
Include `cairooverlay name=overlay` and exactly one video sink; the app draws a
centered crosshair with the GStreamer Cairo overlay plugin.

The pipeline may be supplied with --pipeline (or -p), as a quoted positional
argument, or through GST_PIPELINE. A command-line pipeline takes precedence.
A .env file in the current directory or a parent directory is loaded at startup.
Logging follows RUST_LOG and defaults to info (for example, RUST_LOG=debug).

Examples:
  rs_gst --pipeline 'videotestsrc ! autovideosink'
  GST_PIPELINE='videotestsrc ! autovideosink' rs_gst";

pub enum Action {
    Help,
    Run(Option<String>),
}

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

    Ok(Action::Run(pipeline))
}

#[cfg(test)]
mod tests {
    use super::{Action, parse_args};

    #[test]
    fn accepts_option_and_positional_pipeline() {
        assert!(matches!(
            parse_args(["--pipeline".to_owned(), "videotestsrc ! fakesink".to_owned()]),
            Ok(Action::Run(Some(pipeline))) if pipeline == "videotestsrc ! fakesink"
        ));
        assert!(matches!(
            parse_args(["videotestsrc ! fakesink".to_owned()]),
            Ok(Action::Run(Some(pipeline))) if pipeline == "videotestsrc ! fakesink"
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
