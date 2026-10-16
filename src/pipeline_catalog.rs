//! Loads, searches, and updates named GStreamer pipeline definitions.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CONFIG_DIRECTORY: &str = "rs_gst_overlay";
const PRESETS_FILE: &str = "pipelines.yaml";
const DEFAULT_PRESETS: &str = include_str!("../pipelines.example.yaml");

/// A named pipeline description shown in the picker.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PipelineEntry {
    /// The label shown in the pipeline picker.
    pub(crate) name: String,
    /// The GStreamer launch description to run.
    pub(crate) pipeline: String,
}

/// Named presets available to the player.
pub(crate) struct PipelineCatalog {
    presets: Vec<PipelineEntry>,
    preset_path: Option<PathBuf>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct PresetsDocument {
    presets: Vec<PipelineEntry>,
}

impl PipelineCatalog {
    /// Loads presets from the user's application config directory.
    pub(crate) fn load() -> Self {
        let Some(directory) = config_directory() else {
            log::warn!("Could not find a config directory; pipeline presets will be session-only");
            return Self {
                presets: parse_presets(DEFAULT_PRESETS, "built-in presets").unwrap_or_default(),
                preset_path: None,
            };
        };

        let preset_path = directory.join(PRESETS_FILE);
        let (presets, can_write) = load_presets(&preset_path);

        Self {
            presets,
            preset_path: can_write.then_some(preset_path),
        }
    }

    /// Returns the configured presets.
    pub(crate) fn presets(&self) -> &[PipelineEntry] {
        &self.presets
    }

    /// Reports whether preset changes can be persisted to the user's config file.
    pub(crate) fn can_persist_changes(&self) -> bool {
        self.preset_path.is_some()
    }

    /// Finds the preset whose pipeline description matches the supplied text.
    pub(crate) fn find_preset(&self, pipeline: &str) -> Option<PipelineEntry> {
        self.presets
            .iter()
            .find(|entry| entry.pipeline == pipeline)
            .cloned()
    }

    /// Adds an unlisted startup pipeline to the preset file and returns its entry.
    pub(crate) fn ensure_startup_preset(&mut self, pipeline: &str) -> PipelineEntry {
        if let Some(entry) = self.find_preset(pipeline) {
            log::info!("Using existing pipeline preset '{}'", entry.name);
            return entry;
        }

        let name = unique_preset_name(&self.presets, current_timestamp_seconds());
        let entry = PipelineEntry {
            name,
            pipeline: pipeline.to_owned(),
        };
        self.presets.insert(0, entry.clone());

        if let Some(path) = &self.preset_path {
            match write_presets(path, &self.presets) {
                Ok(()) => log::info!("Added startup pipeline preset '{}'", entry.name),
                Err(error) => log::warn!("Could not save the startup pipeline preset: {error}"),
            }
        } else {
            log::warn!("The startup pipeline preset is available for this session only");
        }

        entry
    }

    /// Renames or edits a preset and persists the updated list.
    pub(crate) fn update_preset(
        &mut self,
        original_name: &str,
        original_pipeline: &str,
        name: &str,
        pipeline: &str,
    ) -> Result<PipelineEntry, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("preset name cannot be empty".to_owned());
        }
        if pipeline.trim().is_empty() {
            return Err("pipeline string cannot be empty".to_owned());
        }

        let index = self
            .presets
            .iter()
            .position(|entry| entry.name == original_name && entry.pipeline == original_pipeline)
            .ok_or_else(|| "the selected preset is no longer available".to_owned())?;
        if self
            .presets
            .iter()
            .enumerate()
            .any(|(other_index, entry)| other_index != index && entry.pipeline == pipeline)
        {
            return Err("another preset already uses this pipeline string".to_owned());
        }
        if self.presets.iter().enumerate().any(|(other_index, entry)| {
            other_index != index && entry.name.eq_ignore_ascii_case(name)
        }) {
            return Err("another preset already uses this name".to_owned());
        }

        let entry = PipelineEntry {
            name: name.to_owned(),
            pipeline: pipeline.to_owned(),
        };
        let mut updated_presets = self.presets.clone();
        updated_presets[index] = entry.clone();
        let Some(path) = &self.preset_path else {
            return Err("the preset file is not available for editing".to_owned());
        };
        write_presets(path, &updated_presets)?;
        self.presets = updated_presets;
        log::info!("Updated pipeline preset '{}'", entry.name);

        Ok(entry)
    }

    /// Removes a named preset and persists the remaining list.
    pub(crate) fn delete_preset(&mut self, name: &str, pipeline: &str) -> Result<(), String> {
        let index = self
            .presets
            .iter()
            .position(|entry| entry.name == name && entry.pipeline == pipeline)
            .ok_or_else(|| "the selected preset is no longer available".to_owned())?;
        let mut updated_presets = self.presets.clone();
        let removed = updated_presets.remove(index);
        let Some(path) = &self.preset_path else {
            return Err("the preset file is not available for editing".to_owned());
        };
        write_presets(path, &updated_presets)?;
        self.presets = updated_presets;
        log::info!("Deleted pipeline preset '{}'", removed.name);

        Ok(())
    }
}

/// Resolves the app config directory, including an override useful for portable installs.
fn config_directory() -> Option<PathBuf> {
    if let Some(directory) = env::var_os("RS_GST_OVERLAY_CONFIG_DIR") {
        return Some(PathBuf::from(directory));
    }

    #[cfg(target_os = "windows")]
    let base_directory = env::var_os("APPDATA").map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base_directory =
        env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"));

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base_directory = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));

    base_directory.map(|directory| directory.join(CONFIG_DIRECTORY))
}

/// Reads presets, creating the config directory and copying built-in examples on first run.
fn load_presets(path: &Path) -> (Vec<PipelineEntry>, bool) {
    match fs::read_to_string(path) {
        Ok(contents) => match parse_presets(&contents, &path.display().to_string()) {
            Some(presets) => (presets, true),
            None => (Vec::new(), false),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let can_write = match path.parent().map(fs::create_dir_all) {
                Some(Err(error)) => {
                    log::warn!("Could not create config directory: {error}");
                    false
                }
                _ => match fs::write(path, DEFAULT_PRESETS) {
                    Ok(()) => true,
                    Err(error) => {
                        log::warn!("Could not create default pipeline presets: {error}");
                        false
                    }
                },
            };
            (
                parse_presets(DEFAULT_PRESETS, "built-in presets").unwrap_or_default(),
                can_write,
            )
        }
        Err(error) => {
            log::warn!(
                "Could not read pipeline presets at {}: {error}",
                path.display()
            );
            (
                parse_presets(DEFAULT_PRESETS, "built-in presets").unwrap_or_default(),
                false,
            )
        }
    }
}

/// Parses preset entries from a YAML document and reports malformed files.
fn parse_presets(contents: &str, source: &str) -> Option<Vec<PipelineEntry>> {
    match serde_yaml_ng::from_str::<PresetsDocument>(contents) {
        Ok(document) => Some(document.presets),
        Err(error) => {
            log::warn!("Could not parse pipeline presets from {source}: {error}");
            None
        }
    }
}

/// Writes the complete preset document after a catalog change.
fn write_presets(path: &Path, presets: &[PipelineEntry]) -> Result<(), String> {
    let contents = serde_yaml_ng::to_string(&PresetsDocument {
        presets: presets.to_vec(),
    })
    .map_err(|error| format!("could not serialize {}: {error}", path.display()))?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Returns a unique date-based name for a newly encountered pipeline.
fn unique_preset_name(presets: &[PipelineEntry], timestamp_seconds: u64) -> String {
    let base_name = format_pipeline_name(timestamp_seconds);
    if !presets
        .iter()
        .any(|entry| entry.name.eq_ignore_ascii_case(&base_name))
    {
        return base_name;
    }

    let mut suffix = 2;
    loop {
        let name = format!("{base_name} ({suffix})");
        if !presets
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(&name))
        {
            return name;
        }
        suffix += 1;
    }
}

/// Formats a Unix timestamp as a readable UTC pipeline preset name.
fn format_pipeline_name(timestamp_seconds: u64) -> String {
    const SECONDS_PER_DAY: u64 = 86_400;
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let days_since_epoch = (timestamp_seconds / SECONDS_PER_DAY) as i64;
    let second_of_day = timestamp_seconds % SECONDS_PER_DAY;
    let shifted_days = days_since_epoch + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    let hour = (second_of_day / 3_600) as u32;
    let minute = ((second_of_day % 3_600) / 60) as u32;
    let hour_12 = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    let period = if hour < 12 { "AM" } else { "PM" };

    format!(
        "{} {}, {} {hour_12}:{minute:02} {period} UTC",
        MONTHS[(month - 1) as usize],
        day,
        year
    )
}

/// Returns the current Unix timestamp in seconds, falling back to the epoch on clock error.
fn current_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
/// Tests YAML preset loading.
mod tests {
    use super::PresetsDocument;

    #[test]
    /// Loads named GStreamer pipelines from the YAML configuration format.
    fn reads_named_pipeline_presets() {
        let document: PresetsDocument = serde_yaml_ng::from_str(
            "presets:\n  - name: Test pattern\n    pipeline: 'videotestsrc ! appsink name=video_sink'\n",
        )
        .unwrap();

        assert_eq!(document.presets.len(), 1);
        assert_eq!(document.presets[0].name, "Test pattern");
        assert_eq!(
            document.presets[0].pipeline,
            "videotestsrc ! appsink name=video_sink"
        );
    }

    #[test]
    /// Keeps the bundled first-run preset valid and usable by the player.
    fn bundled_preset_includes_the_required_sink() {
        let presets = super::parse_presets(super::DEFAULT_PRESETS, "test fixture").unwrap();

        assert!(!presets.is_empty());
        assert!(presets[0].pipeline.contains("appsink name=video_sink"));
    }

    #[test]
    /// Uses a readable UTC date and time for automatic preset names.
    fn formats_auto_preset_name_as_a_friendly_utc_date() {
        assert_eq!(super::format_pipeline_name(0), "Jan 1, 1970 12:00 AM UTC");
        assert_eq!(
            super::format_pipeline_name(1_709_164_800),
            "Feb 29, 2024 12:00 AM UTC"
        );
    }

    #[test]
    /// Keeps automatic preset names unique when several pipelines launch together.
    fn automatic_preset_names_get_a_suffix_when_the_timestamp_repeats() {
        let presets = vec![
            super::PipelineEntry {
                name: "jan 1, 1970 12:00 am utc".to_owned(),
                pipeline: "first ! appsink".to_owned(),
            },
            super::PipelineEntry {
                name: "JAN 1, 1970 12:00 AM UTC (2)".to_owned(),
                pipeline: "second ! appsink".to_owned(),
            },
        ];

        assert_eq!(
            super::unique_preset_name(&presets, 0),
            "Jan 1, 1970 12:00 AM UTC (3)"
        );
    }

    #[test]
    /// Persists an unknown launch pipeline once and reuses an exact string match.
    fn startup_pipeline_persists_deduplicates_edits_and_deletes() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "rs_gst_overlay_preset_test_{}_{}",
            std::process::id(),
            unique_id
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pipelines.yaml");
        fs::write(&path, "presets: []\n").unwrap();
        let mut catalog = super::PipelineCatalog {
            presets: Vec::new(),
            preset_path: Some(path.clone()),
        };

        let pipeline = "videotestsrc ! appsink name=video_sink";
        let added = catalog.ensure_startup_preset(pipeline);
        let persisted =
            super::parse_presets(&fs::read_to_string(&path).unwrap(), "test file").unwrap();
        let reused = catalog.ensure_startup_preset(pipeline);
        let duplicate = super::PipelineEntry {
            name: "Same pipeline under another name".to_owned(),
            pipeline: pipeline.to_owned(),
        };
        catalog.presets.insert(0, duplicate.clone());
        let edited = catalog
            .update_preset(
                &added.name,
                &added.pipeline,
                "Camera preset",
                "rtspsrc location=rtsp://camera ! appsink name=video_sink",
            )
            .unwrap();
        let edited_file =
            super::parse_presets(&fs::read_to_string(&path).unwrap(), "test file").unwrap();
        assert_eq!(edited_file, vec![duplicate.clone(), edited.clone()]);
        catalog
            .delete_preset(&edited.name, &edited.pipeline)
            .unwrap();
        let after_delete =
            super::parse_presets(&fs::read_to_string(&path).unwrap(), "test file").unwrap();

        assert_eq!(added.pipeline, pipeline);
        assert_eq!(persisted, vec![added.clone()]);
        assert_eq!(reused, added);
        assert_eq!(edited.name, "Camera preset");
        assert_eq!(
            edited.pipeline,
            "rtspsrc location=rtsp://camera ! appsink name=video_sink"
        );
        assert_eq!(after_delete, vec![duplicate.clone()]);
        assert_eq!(catalog.presets, vec![duplicate]);

        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    /// Does not rewrite a preset file when it contains fields this version cannot preserve.
    fn leaves_unrecognized_preset_file_content_untouched() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "rs_gst_overlay_unknown_preset_test_{}_{}",
            std::process::id(),
            unique_id
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pipelines.yaml");
        let original_contents = "presets: []\ncustom: keep this field\n";
        fs::write(&path, original_contents).unwrap();

        let (presets, can_write) = super::load_presets(&path);

        assert!(presets.is_empty());
        assert!(!can_write);
        assert_eq!(fs::read_to_string(&path).unwrap(), original_contents);

        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    /// Keeps built-in presets available but disables edits when config creation fails.
    fn missing_preset_file_with_unavailable_parent_is_session_only() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let blocker = std::env::temp_dir().join(format!(
            "rs_gst_overlay_config_blocker_{}_{}",
            std::process::id(),
            unique_id
        ));
        fs::write(&blocker, "not a directory").unwrap();
        let path = blocker.join("pipelines.yaml");

        let (presets, can_write) = super::load_presets(&path);

        assert!(!presets.is_empty());
        assert!(!can_write);
        assert!(blocker.is_file());
        fs::remove_file(blocker).unwrap();
    }
}
