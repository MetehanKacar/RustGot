//! Purpur compatibility: reads Purpur's `purpur.yml` so admins can move a Purpur server directory
//! to `RustGot` as is.
//!
//! The settings `RustGot` applies are typed in [`GlobalSettings`] and [`WorldSettings`]. Everything
//! else in the file is kept when the file is rewritten, and non-default values `RustGot` ignores
//! are listed once at startup.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod minimessage;
mod settings;
mod tree;

use std::{fmt::Write, fs, path::Path, sync::OnceLock};

use serde_norway::{Mapping, Value};
use tracing::{error, info, warn};

use settings::Reader;
pub use settings::{
    Blocks, GameplayMechanics, GlobalSettings, Hunger, Messages, MobEffects, PlayerMechanics,
    WorldSettings,
};
use tree::{ATOMIC_GLOBAL_MAPS, ATOMIC_WORLD_MAPS};

/// File name of the Purpur configuration, relative to the server directory.
pub const CONFIG_FILE: &str = "purpur.yml";
/// `config-version` written by Purpur 26.3.
pub const CONFIG_VERSION: i64 = 50;

/// `purpur.yml` exactly as Purpur 26.3 build 2657 writes it on first start; the only source of
/// Purpur's defaults. Regenerate it by starting a new Purpur build in an empty directory.
const DEFAULT_FILE: &str = include_str!("default_purpur.yml");

/// Purpur keys world sections by dimension; before `config-version` 48 it used the world folder
/// names, which are the level name plus these suffixes.
const DIMENSIONS: [(&str, &str); 3] = [
    ("minecraft:overworld", ""),
    ("minecraft:the_nether", "_nether"),
    ("minecraft:the_end", "_the_end"),
];
const FIRST_VERSION_WITH_DIMENSION_KEYS: i64 = 48;

const DEFAULT_WORLD: &str = "default";
const MAX_LISTED_UNSUPPORTED: usize = 20;

fn builtin() -> &'static Value {
    static BUILTIN: OnceLock<Value> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        serde_norway::from_str(DEFAULT_FILE).unwrap_or_else(|err| {
            error!("Built-in Purpur defaults are invalid: {err}");
            Value::Null
        })
    })
}

fn builtin_world() -> &'static Value {
    tree::get(builtin(), "world-settings.default").unwrap_or(&Value::Null)
}

/// Settings read from `purpur.yml`.
#[derive(Debug, Clone, PartialEq)]
pub struct PurpurConfig {
    /// Global settings.
    pub settings: GlobalSettings,
    default_world: WorldSettings,
    /// Worlds with their own `world-settings.<world>` section.
    worlds: Vec<(String, WorldSettings)>,
}

impl Default for PurpurConfig {
    fn default() -> Self {
        Self::from_tree(builtin(), CONFIG_VERSION, "").0
    }
}

impl PurpurConfig {
    /// Loads `purpur.yml` from `dir`, writing Purpur's default file if there is none and adding
    /// missing keys to an existing one. Falls back to Purpur's defaults on parse errors.
    ///
    /// `level_name` maps world sections of files older than `config-version` 48.
    #[must_use]
    pub fn load(dir: &Path, level_name: &str) -> Self {
        let path = dir.join(CONFIG_FILE);
        let Ok(content) = fs::read_to_string(&path) else {
            match fs::write(&path, DEFAULT_FILE.replace("\r\n", "\n")) {
                Ok(()) => info!("Created default {CONFIG_FILE}"),
                Err(err) => warn!("Couldn't write {}: {err}", path.display()),
            }
            return Self::default();
        };

        let mut root = match serde_norway::from_str::<Value>(&content) {
            Ok(Value::Null) => Value::Mapping(Mapping::new()),
            Ok(root @ Value::Mapping(_)) => root,
            Ok(_) => {
                error!("Couldn't parse {CONFIG_FILE}: it is not a map. Using Purpur defaults.");
                return Self::default();
            }
            Err(err) => {
                error!("Couldn't parse {CONFIG_FILE}: {err}. Using Purpur defaults.");
                return Self::default();
            }
        };

        let version = tree::get(&root, "config-version")
            .and_then(Value::as_i64)
            .unwrap_or(CONFIG_VERSION);
        if version == CONFIG_VERSION {
            if add_missing_defaults(&mut root) {
                save(&path, &root);
            }
        } else {
            warn!(
                "{CONFIG_FILE} has config-version {version}; RustGot reads version {CONFIG_VERSION} and does not migrate other versions"
            );
        }

        let (config, unsupported) = Self::from_tree(&root, version, level_name);
        if !unsupported.is_empty() {
            let mut list = unsupported
                .iter()
                .take(MAX_LISTED_UNSUPPORTED)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            if unsupported.len() > MAX_LISTED_UNSUPPORTED {
                let _ = write!(
                    list,
                    " and {} more",
                    unsupported.len() - MAX_LISTED_UNSUPPORTED
                );
            }
            warn!(
                "{CONFIG_FILE}: {} non-default setting(s) are not supported by RustGot yet and have no effect: {list}",
                unsupported.len()
            );
        }
        config
    }

    /// Resolves the typed settings and lists the non-default settings `RustGot` does not apply.
    fn from_tree(root: &Value, version: i64, level_name: &str) -> (Self, Vec<String>) {
        let builtin_world = builtin_world();

        let mut reader = Reader::new(vec![(Some(String::new()), root)], builtin());
        let settings = GlobalSettings::read(&mut reader);
        let global_supported = reader.supported;

        let user_default = tree::get(root, "world-settings.default");
        let mut reader = Reader::new(
            user_default
                .map(|section| (Some("world-settings.default.".to_owned()), section))
                .into_iter()
                .collect(),
            builtin_world,
        );
        let default_world = WorldSettings::read(&mut reader);
        let world_supported = reader.supported;

        let mut unsupported = Vec::new();
        let mut worlds = Vec::new();
        for (key, value) in root.as_mapping().into_iter().flatten() {
            let Some(key) = key.as_str() else {
                continue;
            };
            match key {
                "config-version" => {}
                "world-settings" => {
                    for (name, section) in value.as_mapping().into_iter().flatten() {
                        let Some(name) = name.as_str() else {
                            continue;
                        };
                        let mut paths = Vec::new();
                        tree::differing_paths(
                            section,
                            builtin_world,
                            "",
                            ATOMIC_WORLD_MAPS,
                            &mut paths,
                        );
                        unsupported.extend(
                            paths
                                .into_iter()
                                .filter(|path| !world_supported.contains(&path.as_str()))
                                .map(|path| format!("world-settings.{name}.{path}")),
                        );
                        if name == DEFAULT_WORLD {
                            continue;
                        }
                        let mut sources = vec![(Some(format!("world-settings.{name}.")), section)];
                        // Warnings for the default section were logged above.
                        sources.extend(user_default.map(|section| (None, section)));
                        let mut reader = Reader::new(sources, builtin_world);
                        worlds.push((
                            world_key(name, version, level_name),
                            WorldSettings::read(&mut reader),
                        ));
                    }
                }
                _ => {
                    let Some(default) = tree::get(builtin(), key) else {
                        continue;
                    };
                    let mut paths = Vec::new();
                    tree::differing_paths(value, default, key, ATOMIC_GLOBAL_MAPS, &mut paths);
                    unsupported.extend(
                        paths
                            .into_iter()
                            .filter(|path| !global_supported.contains(&path.as_str())),
                    );
                }
            }
        }

        let config = Self {
            settings,
            default_world,
            worlds,
        };
        (config, unsupported)
    }

    /// Settings of the world with Purpur's key `name`, such as `minecraft:the_nether`.
    #[must_use]
    pub fn world(&self, name: &str) -> &WorldSettings {
        self.worlds
            .iter()
            .find(|(world, _)| world == name)
            .map_or(&self.default_world, |(_, settings)| settings)
    }
}

/// Purpur's key for a world section; files older than version 48 name sections after folders.
fn world_key(section: &str, version: i64, level_name: &str) -> String {
    if version < FIRST_VERSION_WITH_DIMENSION_KEYS
        && let Some((key, _)) = DIMENSIONS
            .iter()
            .find(|(_, suffix)| section.strip_suffix(suffix) == Some(level_name))
    {
        return (*key).to_owned();
    }
    section.to_owned()
}

/// Adds the global keys and `world-settings.default` keys Purpur would add. Per-world sections
/// only hold overrides, so they are left alone.
fn add_missing_defaults(root: &mut Value) -> bool {
    let (Value::Mapping(root), Value::Mapping(defaults)) = (root, builtin()) else {
        return false;
    };
    let mut global_defaults = defaults.clone();
    global_defaults.remove("world-settings");
    let mut changed = tree::merge_missing(root, &global_defaults, "", ATOMIC_GLOBAL_MAPS);

    let Value::Mapping(builtin_world) = builtin_world() else {
        return changed;
    };
    let world_settings = root
        .entry(Value::from("world-settings"))
        .or_insert_with(|| {
            changed = true;
            Value::Mapping(Mapping::new())
        });
    if let Value::Mapping(world_settings) = world_settings {
        let default_world = world_settings
            .entry(Value::from(DEFAULT_WORLD))
            .or_insert_with(|| {
                changed = true;
                Value::Mapping(Mapping::new())
            });
        if let Value::Mapping(default_world) = default_world {
            changed |= tree::merge_missing(default_world, builtin_world, "", ATOMIC_WORLD_MAPS);
        }
    }
    changed
}

/// Writes `root` with Purpur's header comment.
fn save(path: &Path, root: &Value) {
    let header = DEFAULT_FILE
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    match serde_norway::to_string(root) {
        Ok(yaml) => {
            if let Err(err) = fs::write(path, format!("{header}\n\n{yaml}")) {
                warn!("Couldn't write {}: {err}", path.display());
            }
        }
        Err(err) => warn!("Couldn't serialize {CONFIG_FILE}: {err}"),
    }
}

static CONFIG: OnceLock<PurpurConfig> = OnceLock::new();

/// Makes `config` the settings returned by [`get`] and [`world`]. Call once at startup.
pub fn init(config: PurpurConfig) {
    if CONFIG.set(config).is_err() {
        warn!("{CONFIG_FILE} was already loaded; ignoring the reload");
    }
}

/// The loaded settings, or Purpur's defaults before [`init`].
#[must_use]
pub fn get() -> &'static PurpurConfig {
    CONFIG.get_or_init(PurpurConfig::default)
}

/// Settings of the world with Purpur's key `name` (the dimension, such as `minecraft:overworld`).
#[must_use]
pub fn world(name: &str) -> &'static WorldSettings {
    get().world(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Value {
        serde_norway::from_str(yaml).unwrap()
    }

    #[test]
    fn every_supported_setting_has_a_purpur_default() {
        let mut reader = Reader::new(Vec::new(), builtin());
        GlobalSettings::read(&mut reader);
        assert_eq!(reader.missing_defaults, Vec::<&str>::new());
        let mut reader = Reader::new(Vec::new(), builtin_world());
        WorldSettings::read(&mut reader);
        assert_eq!(reader.missing_defaults, Vec::<&str>::new());
    }

    #[test]
    fn rewriting_keeps_purpur_default_values() {
        // Drop keys so the file gets rewritten through the YAML serializer, then compare the
        // result with Purpur's file key for key (quoted numbers, float digits, unicode, ...).
        let mut root = builtin().clone();
        let Value::Mapping(map) = &mut root else {
            panic!("defaults are a map");
        };
        map.remove("verbose");
        let world = map
            .get_mut("world-settings")
            .and_then(|worlds| worlds.get_mut("default"))
            .and_then(Value::as_mapping_mut)
            .unwrap();
        world.remove("hunger");
        world.remove("blocks");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, serde_norway::to_string(&root).unwrap()).unwrap();
        let _ = PurpurConfig::load(dir.path(), "world");

        let rewritten = fs::read_to_string(&path).unwrap();
        assert!(rewritten.starts_with("# This is the main configuration file for Purpur."));
        assert_eq!(&parse(&rewritten), builtin());
    }

    #[test]
    fn writes_purpur_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = PurpurConfig::load(dir.path(), "world");
        assert_eq!(config, PurpurConfig::default());
        let written = fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
        assert_eq!(written, DEFAULT_FILE.replace("\r\n", "\n"));
    }

    #[test]
    fn rewrite_keeps_unknown_keys_and_admin_maps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(
            &path,
            "config-version: 50
my-plugin-key: 1
settings:
  old-removed-option: true
  block-fall-multipliers:
    minecraft:hay_block:
      damage: 0.1
world-settings:
  default:
    mobs:
      zombie:
        some-future-option: 3
  minecraft:the_nether:
    blocks:
      dragon_egg:
        teleport: false
",
        )
        .unwrap();
        let _ = PurpurConfig::load(dir.path(), "world");
        let root = parse(&fs::read_to_string(&path).unwrap());

        assert_eq!(tree::get(&root, "my-plugin-key"), Some(&Value::from(1)));
        assert_eq!(
            tree::get(&root, "settings.old-removed-option"),
            Some(&Value::from(true))
        );
        assert_eq!(
            tree::get(
                &root,
                "world-settings.default.mobs.zombie.some-future-option"
            ),
            Some(&Value::from(3))
        );
        // Added like Purpur would add them.
        assert_eq!(
            tree::get(&root, "settings.server-mod-name"),
            Some(&Value::from("Purpur"))
        );
        assert!(tree::get(&root, "world-settings.default.mobs.zombie.jockey.chance").is_some());
        // Admin maps and per-world sections are not filled with defaults.
        assert_eq!(
            tree::get(&root, "settings.block-fall-multipliers"),
            Some(&parse("minecraft:hay_block:\n  damage: 0.1"))
        );
        assert_eq!(
            tree::get(&root, "world-settings.minecraft:the_nether"),
            Some(&parse("blocks:\n  dragon_egg:\n    teleport: false"))
        );
    }

    #[test]
    fn world_sections_override_the_default_section_key_by_key() {
        let root = parse(
            "world-settings:
  default:
    gameplay-mechanics:
      mob-effects:
        health-regen-amount: 2
        poison-degeneration-amount: 0.5
  minecraft:the_nether:
    gameplay-mechanics:
      mob-effects:
        health-regen-amount: lots
        wither-degeneration-amount: 3.0
",
        );
        let (config, _) = PurpurConfig::from_tree(&root, CONFIG_VERSION, "world");

        let nether = &config
            .world("minecraft:the_nether")
            .gameplay_mechanics
            .mob_effects;
        // A value of the wrong type falls back to the default section, like Bukkit's getters.
        assert_eq!(nether.health_regen_amount, 2.0);
        assert_eq!(nether.poison_degeneration_amount, 0.5);
        assert_eq!(nether.wither_degeneration_amount, 3.0);
        assert_eq!(nether.hunger_exhaustion_amount, 0.005);

        let overworld = &config
            .world("minecraft:overworld")
            .gameplay_mechanics
            .mob_effects;
        assert_eq!(overworld.health_regen_amount, 2.0);
        assert_eq!(overworld.wither_degeneration_amount, 1.0);
    }

    #[test]
    fn old_files_name_world_sections_after_folders() {
        let root = parse(
            "world-settings:
  survival_nether:
    hunger:
      starvation-damage: 4
",
        );
        let (config, _) = PurpurConfig::from_tree(&root, 47, "survival");
        assert_eq!(
            config
                .world("minecraft:the_nether")
                .hunger
                .starvation_damage,
            4.0
        );
        let (config, _) = PurpurConfig::from_tree(&root, CONFIG_VERSION, "survival");
        assert_eq!(
            config
                .world("minecraft:the_nether")
                .hunger
                .starvation_damage,
            1.0
        );
    }

    #[test]
    fn lists_only_unsupported_settings_that_differ_from_defaults() {
        let root = parse(
            "verbose: false
unknown-key: 5
settings:
  use-alternate-keepalive: true
  tps-catchup: true
  server-mod-name: MyServer
  blast-resistance-overrides:
    minecraft:obsidian: 5.0
world-settings:
  default:
    gameplay-mechanics:
      use-better-mending: true
      entity-lifespan: 0.0
      mob-effects:
        health-regen-amount: 3.0
  minecraft:the_end:
    blocks:
      bed:
        explode: false
      dragon_egg:
        teleport: false
",
        );
        let (_, unsupported) = PurpurConfig::from_tree(&root, CONFIG_VERSION, "world");
        assert_eq!(
            unsupported,
            [
                "settings.use-alternate-keepalive",
                "settings.blast-resistance-overrides",
                "world-settings.default.gameplay-mechanics.use-better-mending",
                "world-settings.minecraft:the_end.blocks.bed.explode",
            ]
        );
    }
}
