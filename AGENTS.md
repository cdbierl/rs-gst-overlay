# Repository guidance

## Project overview

`rs_gst_overlay` is a Rust 2024 desktop viewer for video from GStreamer
pipelines. The README contains platform dependencies, run examples, pipeline
syntax, and preset configuration details; consult it before changing setup or
user-facing behavior.

## Source layout

- `src/main.rs` initializes `.env` and logging, resolves pipeline input, and
  starts the application.
- `src/cli.rs` parses command-line arguments and owns the concise `--help`
  text.
- `src/pipeline.rs` parses GStreamer launch descriptions.
- `src/pipeline_catalog.rs` loads and updates named YAML presets.
- `src/stream.rs` owns the GStreamer worker, appsink, frame transfer, playback
  state, and pipeline restart behavior.
- `src/player.rs` owns the egui window, controls, texture upload, and view
  transforms.
- `src/overlay.rs` owns guide painting and RGBA texture filters.

Keep code with the module that owns its behavior. Prefer a focused module for a
growing responsibility over a generic `utils` module.

## Important behavior

- A video pipeline must provide an `appsink` named `video_sink`. The player
  negotiates RGBA frames and displays the newest available frame.
- Keep blocking GStreamer work and appsink reads on the stream worker, not in
  the egui update loop. Send playback changes to the worker through its command
  channel.
- Preserve the original RGBA frame when applying display filters so users can
  switch back to source colors. Pixel filters must leave alpha unchanged.
- Guide overlays are painted over the displayed video; texture filters are
  applied to frame pixels. Keep these responsibilities distinct when adding
  modes.
- Pipeline presets use a strict `presets` YAML document with `name` and
  `pipeline` entries. Startup pipeline strings are added to the user's preset
  catalog when they are not already present; if the config directory is
  unavailable, the entry is available for the current session only.
- Use placeholder hosts and paths in examples. Do not commit credentials,
  private camera addresses, or machine-specific `.env` settings.

## Development and validation

Use the project's installed Rust toolchain and native GStreamer dependencies.
Run these checks after Rust changes:

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

The stream tests use GStreamer test plugins such as `videotestsrc`,
`videoconvert`, and `appsink`. If a test fails because a native plugin is
missing, verify the local GStreamer installation before changing the test.

Add focused tests for pure parsing, preset, image-filter, and coordinate
transform behavior. Keep UI tests for behavior that can be verified without
duplicating egui implementation details. Do not add dependencies for small
helpers that can be expressed with the existing Rust and egui APIs.
