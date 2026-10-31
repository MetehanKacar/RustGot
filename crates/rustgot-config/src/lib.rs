//! `RustGot`-specific configuration, stored in `rustgot.yml` next to `pumpkin.toml`.
//!
//! Upstream Pumpkin settings stay in `pumpkin.toml`; everything `RustGot` adds on top
//! (the Java plugin bridge, Purpur compatibility, ...) lives here so the fork's diff
//! against upstream stays small.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::{error, info, warn};

/// File name of the `RustGot` configuration, relative to the server directory.
pub const CONFIG_FILE: &str = "rustgot.yml";

/// Root of `rustgot.yml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct RustGotConfig {
    /// Settings for the embedded JVM that runs Paper/Bukkit `.jar` plugins.
    pub jvm: JvmConfig,
}

/// Settings for the embedded JVM.
///
/// The JVM is started lazily: if `plugins/` contains no `.jar` files it is never started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct JvmConfig {
    /// Allow loading Java plugins at all.
    pub enabled: bool,
    /// Download `GraalVM` Community JDK when no suitable Java is installed.
    pub auto_download: bool,
    /// Minimum Java feature version required by the plugin bridge.
    pub min_java_version: u32,
    /// Maximum JVM heap in MiB. `0` sizes it automatically from the plugin count.
    pub max_heap_mb: u32,
}

impl Default for JvmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_download: true,
            min_java_version: 25,
            max_heap_mb: 0,
        }
    }
}

impl RustGotConfig {
    /// Loads `rustgot.yml` from `dir`, writing a default file if it does not exist
    /// and filling in any missing keys. Falls back to defaults on parse errors.
    #[must_use]
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        let Ok(content) = fs::read_to_string(&path) else {
            let config = Self::default();
            config.save(&path);
            info!("Created default {CONFIG_FILE}");
            return config;
        };

        let config: Self = match serde_norway::from_str(&content) {
            Ok(config) => config,
            Err(err) => {
                error!("Couldn't parse {CONFIG_FILE}: {err}. Using default config.");
                return Self::default();
            }
        };

        // Rewrite the file if keys were missing, so users can see every option.
        let written: Result<serde_norway::Value, _> = serde_norway::from_str(&content);
        let full = serde_norway::to_value(&config);
        if let (Ok(written), Ok(full)) = (written, full)
            && written != full
        {
            config.save(&path);
        }
        config
    }

    fn save(&self, path: &Path) {
        match serde_norway::to_string(self) {
            Ok(yaml) => {
                if let Err(err) = fs::write(path, yaml) {
                    warn!("Couldn't write {}: {err}", path.display());
                }
            }
            Err(err) => warn!("Couldn't serialize {CONFIG_FILE}: {err}"),
        }
    }
}

/// Returns the `.jar` files directly inside `plugins_dir` (Paper/Bukkit plugins), sorted.
#[must_use]
pub fn java_plugin_jars(plugins_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(plugins_dir) else {
        return Vec::new();
    };
    let mut jars: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
        .collect();
    jars.sort();
    jars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_default_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = RustGotConfig::load(dir.path());
        assert_eq!(config, RustGotConfig::default());
        assert!(dir.path().join(CONFIG_FILE).exists());
    }

    #[test]
    fn fills_missing_keys_and_keeps_set_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, "jvm:\n  auto-download: false\n").unwrap();

        let config = RustGotConfig::load(dir.path());
        assert!(!config.jvm.auto_download);
        assert_eq!(config.jvm.min_java_version, 25);

        let rewritten = fs::read_to_string(&path).unwrap();
        assert!(rewritten.contains("min-java-version: 25"));
        assert!(rewritten.contains("auto-download: false"));
    }

    #[test]
    fn finds_only_jar_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.jar"), b"").unwrap();
        fs::write(dir.path().join("a.JAR"), b"").unwrap();
        fs::write(dir.path().join("plugin.wasm"), b"").unwrap();
        fs::create_dir(dir.path().join("dir.jar")).unwrap();

        let names: Vec<_> = java_plugin_jars(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.JAR", "b.jar"]);
        assert!(java_plugin_jars(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn invalid_yaml_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CONFIG_FILE), "jvm: [not, a, map").unwrap();
        assert_eq!(RustGotConfig::load(dir.path()), RustGotConfig::default());
    }
}
