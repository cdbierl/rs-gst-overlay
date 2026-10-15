mod cli;
mod overlay;
mod pipeline;
mod player;

use std::env;
use std::io::ErrorKind;
use std::process::ExitCode;

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

fn run() -> Result<(), String> {
    let action = cli::parse_args(env::args().skip(1))?;
    let (pipeline_arg, no_overlay) = match action {
        cli::Action::Help => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        cli::Action::Run {
            pipeline,
            no_overlay,
        } => (pipeline, no_overlay),
    };
    let description = match pipeline_arg {
        Some(description) => description,
        None => env::var("GST_PIPELINE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "no pipeline provided; use --pipeline or set GST_PIPELINE".to_owned())?,
    };

    gstreamer::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
    let pipeline = pipeline::parse(&description)?;
    if no_overlay {
        log::info!("Crosshair display disabled");
    }
    player::play(pipeline, !no_overlay)
}
