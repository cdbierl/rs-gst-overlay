# rs_gst_overlay

`rs_gst_overlay` plays a GStreamer pipeline and, by default, draws a centered
green crosshair over the video passing through a `cairooverlay` element. Pass
`--no-overlay` to play a pipeline without configuring a crosshair. The
application configures an existing overlay in the pipeline; it does not add an
overlay or a video sink for you.

## Requirements

- Rust and Cargo
- GStreamer development libraries and the GStreamer Rust bindings' native
  dependencies
- The GStreamer `cairooverlay` plugin unless `--no-overlay` is used
- A video sink plugin suitable for your system, such as `autovideosink`
- The source, decoder, and converter plugins required by your pipeline

Plugin names and packages vary by operating system. If GStreamer reports that
an element such as `cairooverlay` or `autovideosink` cannot be found, install
the GStreamer plugin set that provides that element for your system.

## Build and run

Build the application with Cargo:

```sh
cargo build
```

Run a test pattern with a video sink:

```sh
cargo run -- --pipeline 'videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink'
```

To play without drawing the crosshair, use `--no-overlay`. In this mode the
pipeline does not need a `cairooverlay` element:

```sh
cargo run -- --no-overlay --pipeline 'videotestsrc ! autovideosink'
```

The executable can also be run directly after building:

```sh
./target/debug/rs_gst_overlay --pipeline 'videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink'
```

Use `--help` to see the command-line usage and examples:

```sh
cargo run -- --help
```

## Pipeline input

Pass a GStreamer pipeline description with `--pipeline` or `-p`, as one quoted
positional argument, or through `GST_PIPELINE`. A command-line pipeline takes
precedence over `GST_PIPELINE`.

```sh
cargo run -- --pipeline 'videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink'
cargo run -- 'videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink'
GST_PIPELINE='videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink' cargo run
```

Unless `--no-overlay` is specified, the pipeline must contain an element named
exactly `overlay`, and that element must be a `cairooverlay`. Place it on the
video path where the crosshair should be drawn, before the sink that displays
that path. The application does not require a specific number of video sinks.

For example, this pipeline sends video through the overlay and then to a
display sink:

```text
videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink
```

The pipeline runs until it reaches end-of-stream or GStreamer reports an
error. Live sources normally do not reach end-of-stream on their own.

## Crosshair and logging

The default overlay is green with 85% opacity and a 4-pixel stroke. It draws
crosshair lines spanning 85% of the video width and height, plus an outlined
circle with a 50-pixel radius at their center. These values are currently set
in the application.

Logging uses `RUST_LOG` and defaults to `info`. For example:

```sh
RUST_LOG=debug cargo run -- --pipeline 'videotestsrc ! videoconvert ! cairooverlay name=overlay ! autovideosink'
```

If a `.env` file exists in the current directory or a parent directory, it is
loaded at startup. It can be used to set `GST_PIPELINE` and logging options.

## Tests

Run the test suite with:

```sh
cargo test
```

Some GStreamer integration tests are skipped when the required runtime plugins
are unavailable.
