# rs_gst_overlay

`rs_gst_overlay` displays video from a GStreamer pipeline in a native egui
window. The pipeline must include an `appsink` named `video_sink`. The viewer
negotiates RGBA frames from that sink, shows the latest frame, and draws a
center crosshair over the image. A zoom control and the mouse wheel crop the
visible frame for digital zoom.

## Requirements

- Rust and Cargo
- GStreamer development libraries and runtime plugins needed by your pipeline
- The GStreamer app and video libraries
- Native windowing and OpenGL development libraries for eframe on your system

The video source, decoder, converter, and other GStreamer plugins depend on
your pipeline. The app requests RGBA output from `video_sink` and keeps at most
one queued sample so display latency does not build up when rendering falls
behind.

## Build and run

Build the application with Cargo:

```sh
cargo build
```

Run a test pattern in the viewer:

```sh
cargo run -- --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

The executable can also be run directly after building:

```sh
./target/debug/rs_gst_overlay --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

Use `--help` to see command-line usage:

```sh
cargo run -- --help
```

## Pipeline input

Pass a GStreamer pipeline description with `--pipeline` or `-p`, as one quoted
positional argument, or through `GST_PIPELINE`. A command-line pipeline takes
precedence over `GST_PIPELINE`.

```sh
cargo run -- --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
cargo run -- 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
GST_PIPELINE='videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink' cargo run
```

The pipeline must contain an `appsink` named exactly `video_sink`. The viewer
sets its caps to `video/x-raw,format=RGBA`, reads samples on a worker thread,
and passes only the newest copied frame to the window. The pipeline should not
include a display sink for the same video path.

Pipeline playback stops at end-of-stream or when GStreamer reports an error;
the window stays open until it is closed. Live sources normally do not reach
end-of-stream on their own.

## Video controls

Use the Zoom slider or scroll the mouse wheel over the video to zoom from 1× to
8×. Zoom crops around the center of the frame. Reset returns to 1×.

Use Rotate to turn the video 90° clockwise. Flip H and Flip V toggle horizontal
and vertical mirroring.

The crosshair is enabled by default. Toggle it at any time with the Crosshair
checkbox. It is green with 85% opacity and a 4-point stroke. Its lines span
85% of the displayed frame and it has an outlined center circle with a
50-point radius.

## Logging and tests

Logging uses `RUST_LOG` and defaults to `info`:

```sh
RUST_LOG=debug cargo run -- --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

If a `.env` file exists in the current directory or a parent directory, it is
loaded at startup. It can be used to set `GST_PIPELINE` and logging options.

Run the test suite with:

```sh
cargo test
```
