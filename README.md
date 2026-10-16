# rs_gst_overlay

`rs_gst_overlay` is a Rust desktop viewer for video produced by a GStreamer
pipeline. It sends frames from a named `appsink` to a native `egui` window,
where you can choose an overlay and adjust the video's display.

The viewer requests RGBA frames, reads samples on a worker thread, and keeps
only the newest frame for display. The window provides an overlay picker,
zoom, rotation, horizontal and vertical flips, a displayed-frame rate, and
the current pipeline status. The Play/Stop button controls the pipeline and
restarts it from the original description after it has stopped.

## Dependencies

### Rust crates

Cargo downloads and builds these direct dependencies:

| Crate | Version | Purpose |
| --- | --- | --- |
| `eframe` | 0.33 | Native application window and egui integration |
| `egui` | 0.33 | Immediate-mode UI and drawing |
| `egui-material3` | 0.0.9 | Material controls and icon buttons |
| `gstreamer` | 0.23 | GStreamer initialization and pipeline handling |
| `gstreamer-app` | 0.23 | Appsink access |
| `gstreamer-video` | 0.23 | Video caps, frame mapping, and pixel access |
| `dotenvy` | 0.15 | Loading environment settings from `.env` |
| `env_logger` | 0.11 | Logging configuration from `RUST_LOG` |
| `log` | 0.4 | Logging facade |
| `serde` | 1.0 | Serializing and deserializing pipeline presets |
| `serde_yaml_ng` | 0.10 | Reading and writing YAML presets |

### Native dependencies

- Rust and Cargo with support for the 2024 edition.
- GStreamer development libraries, including its app and video libraries.
- GStreamer runtime plugins required by the selected pipeline. The test-pattern
  example and stream worker tests use `videotestsrc`, `videoconvert`, and
  `appsink`.
- Native windowing and OpenGL development libraries required by `eframe` on
  your platform. On Linux, this project enables the X11, Wayland, and Glow
  backends.

The Material Symbols Rounded font is bundled in `assets/` for toolbar icons;
its license information is in [`assets/README.md`](assets/README.md).

## Build and run

Build the application:

```sh
cargo build
```

Run a test pattern:

```sh
cargo run -- --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

Use `--help` to see command-line usage:

```sh
cargo run -- --help
```

## Pipeline input

Pass a GStreamer pipeline description with `--pipeline` or `-p`, using the
`--pipeline=<PIPELINE>` form, as one quoted positional argument, or through
`GST_PIPELINE`. A command-line pipeline takes precedence over `GST_PIPELINE`.
The pipeline is optional: without one, the window opens with no source active
and you can start a preset from the toolbar picker.

```sh
cargo run -- --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
cargo run -- 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
GST_PIPELINE='videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink' cargo run
cargo run
```

### Video file

For a video-only file, use `filesrc` and `decodebin` so GStreamer can select
the file demuxer and decoder:

```sh
cargo run -- --pipeline 'filesrc location="/path/to/video.mp4" ! decodebin ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

The required demuxer and decoder plugins depend on the file format. This simple
form assumes the file has no audio stream; an audio stream also needs to be
linked to an audio sink.

### Network video stream

For an H.264 RTSP camera, use `rtspsrc` with its matching RTP depayloader and
decoder:

```sh
cargo run -- --pipeline 'rtspsrc location=rtsp://camera.example:8554/stream protocols=tcp latency=200 ! application/x-rtp,media=video,encoding-name=H264 ! rtph264depay ! h264parse ! avdec_h264 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

Replace the sample URI with the camera or server address. The RTSP example
requires plugins for `rtspsrc`, `rtph264depay`, `h264parse`, and `avdec_h264`;
use the depayloader, parser, and decoder that match your stream's codec.

The pipeline must contain an `appsink` named exactly `video_sink` and deliver
video to it. The viewer sets that sink's caps to `video/x-raw,format=RGBA`,
copies the newest available frame for display, and drops older queued samples
to limit latency. Do not add a display sink to the same video path.

Playback stops when the pipeline reaches end-of-stream or reports an error. The
window stays open so you can inspect the status and restart playback. Live
sources normally do not reach end-of-stream on their own.

## Pipeline presets

The toolbar's Pipeline picker searches named presets. Selecting an entry
immediately switches the player to that pipeline. Add or edit named presets
directly in `pipelines.yaml`.

When the app starts with a pipeline from `--pipeline`, a positional argument,
or `GST_PIPELINE`, it adds that pipeline to the top of `pipelines.yaml` if no
existing preset has the exact same pipeline string. The generated name uses
the launch date and time in UTC. In the picker, choose **Edit selected
preset…** to change its name or edit the full pipeline string. Saving a
pipeline edit immediately loads the new description. **Delete selected
preset…** removes the entry from the file while leaving playback running. If
that same pipeline is supplied again at a later launch, it will be added again
automatically.

On first launch, the app creates a config directory and a `pipelines.yaml`
file containing a test-pattern preset. The config directory is
`$XDG_CONFIG_HOME/rs_gst_overlay` on Linux (or `~/.config/rs_gst_overlay` when
`XDG_CONFIG_HOME` is unset), `~/Library/Application Support/rs_gst_overlay` on
macOS, and `%APPDATA%/rs_gst_overlay` on Windows. Set
`RS_GST_OVERLAY_CONFIG_DIR` to use a different directory.

Add or edit presets in `pipelines.yaml` using this format:

```yaml
presets:
  - name: Test pattern
    pipeline: >-
      videotestsrc is-live=true ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink
  - name: RTSP camera
    pipeline: >-
      rtspsrc location=rtsp://camera.example:8554/stream protocols=tcp latency=200 ! application/x-rtp,media=video,encoding-name=H264 ! rtph264depay ! h264parse ! avdec_h264 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink
```

## Controls

- Use the Overlay picker to show no overlay, a centered target reticle, or a
  rule-of-thirds grid for checking framing.
- Use the Zoom slider or scroll the mouse wheel over the video to zoom from 1×
  to 10×. Zoom crops around the center of the frame.
- Rotate turns the video 90° clockwise. The horizontal and vertical flip
  buttons mirror the video in their respective directions.
- Use the Play/Stop button to control the pipeline. Play restarts it after an
  error or end-of-stream.
- The bottom status bar shows the rate of newly displayed frames and the
  current pipeline status.

The target reticle uses 85% opacity, a 2-point stroke, lines spanning 85% of
the displayed frame, and a 50-point-radius center circle. The rule-of-thirds
grid uses subtle white lines at one-third and two-thirds of the frame.

## Developing

The main source modules are organized by responsibility:

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Startup, environment loading, logging, and application launch |
| `src/cli.rs` | Command-line argument parsing |
| `src/pipeline.rs` | Parsing GStreamer pipeline descriptions |
| `src/pipeline_catalog.rs` | YAML pipeline presets |
| `src/stream.rs` | Pipeline worker, appsink frame handling, status, and restart logic |
| `src/player.rs` | egui window, toolbar, video display, and view transforms |
| `src/overlay.rs` | Target-reticle and rule-of-thirds overlay painting |

After making changes, run the test suite and code checks:

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Use `RUST_LOG` to change the application log level. It defaults to `info`:

```sh
RUST_LOG=debug cargo run -- --pipeline 'videotestsrc ! videoconvert ! video/x-raw,format=RGBA ! appsink name=video_sink'
```

If a `.env` file exists in the current directory or a parent directory, it is
loaded at startup. Use it for settings such as `GST_PIPELINE` and `RUST_LOG`.
