//! Starts the GStreamer video player and configures its command-line runtime.

mod cli;
mod overlay;
mod pipeline;
mod pipeline_catalog;
mod player;
mod stream;

use std::env;
use std::io::ErrorKind;
use std::process::ExitCode;

/// Initializes logging and configuration, then returns an appropriate process status.
fn main() -> ExitCode {
    let dotenv_result = dotenvy::dotenv();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    match dotenv_result {
        Ok(path) => log::info!("Loaded environment from {}", path.display()),
        Err(dotenvy::Error::Io(error)) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            log::error!("could not load .env: {error}");
            return ExitCode::FAILURE;
        }
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses startup options, initializes GStreamer, and launches the video player.
fn run() -> Result<(), String> {
    let action = cli::parse_args(env::args().skip(1))?;
    let pipeline_arg = match action {
        cli::Action::Help => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        cli::Action::Run { pipeline } => pipeline,
    };
    let description = pipeline_arg.or_else(|| {
        env::var("GST_PIPELINE")
            .ok()
            .filter(|value| !value.trim().is_empty())
    });

    gstreamer::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
    player::play(description)
}
