//! Helpers for walking `purpur.yml` as a YAML value tree with Bukkit's dotted paths.

use serde_norway::{Mapping, Value};

/// Settings Purpur reads as a whole map: when an admin writes the section, the defaults are not
/// merged into it, so missing entries stay missing. Paths are relative to the root.
pub const ATOMIC_GLOBAL_MAPS: &[&str] = &[
    "settings.block-fall-multipliers",
    "settings.blast-resistance-overrides",
];

/// Like [`ATOMIC_GLOBAL_MAPS`], relative to a `world-settings.<world>` section.
pub const ATOMIC_WORLD_MAPS: &[&str] = &[
    "blocks.big_dripleaf.tilt-delay",
    "gameplay-mechanics.minecart.controllable.block-speed",
];

/// Looks up a dotted Bukkit path such as `settings.server-mod-name`.
pub fn get<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(root, |node, key| node.as_mapping()?.get(key))
}

fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

/// `SnakeYAML` (YAML 1.1) reads these plain scalars as booleans; `serde_norway` (YAML 1.2) keeps
/// them as strings.
pub fn yaml11_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Some(true),
        "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => Some(false),
        _ => None,
    }
}

/// Inserts every key of `defaults` that `user` lacks, recursing into nested sections except the
/// atomic maps. Existing values are never changed. Returns whether anything was added.
pub fn merge_missing(
    user: &mut Mapping,
    defaults: &Mapping,
    prefix: &str,
    atomic: &[&str],
) -> bool {
    let mut changed = false;
    for (key, default) in defaults {
        let Some(name) = key.as_str() else {
            continue;
        };
        let path = join(prefix, name);
        match user.get_mut(key) {
            None => {
                user.insert(key.clone(), default.clone());
                changed = true;
            }
            Some(Value::Mapping(section)) => {
                if let Value::Mapping(default_section) = default
                    && !atomic.contains(&path.as_str())
                {
                    changed |= merge_missing(section, default_section, &path, atomic);
                }
            }
            Some(_) => {}
        }
    }
    changed
}

/// Collects the paths under `user` whose value differs from `defaults`. Keys Purpur does not know
/// are skipped because Purpur ignores them too.
pub fn differing_paths(
    user: &Value,
    defaults: &Value,
    prefix: &str,
    atomic: &[&str],
    out: &mut Vec<String>,
) {
    if let (Value::Mapping(user), Value::Mapping(defaults)) = (user, defaults)
        && !atomic.contains(&prefix)
    {
        for (key, value) in user {
            if let (Some(name), Some(default)) = (key.as_str(), defaults.get(key)) {
                differing_paths(value, default, &join(prefix, name), atomic, out);
            }
        }
    } else if !same_setting(user, defaults) {
        out.push(prefix.to_owned());
    }
}

/// Compares two values the way Purpur's typed getters would see them: `1` equals `1.0`, and a
/// scalar equals another scalar with the same text (`'20.0'` and `20.0`).
fn same_setting(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Sequence(a), Value::Sequence(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_setting(a, b))
        }
        (Value::Mapping(a), Value::Mapping(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same_setting(a, b)))
        }
        _ => match (scalar_string(a), scalar_string(b)) {
            (Some(a), Some(b)) => a == b,
            _ => a == b,
        },
    }
}

/// Bukkit's `getString`: any scalar converted to its text.
pub fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}
