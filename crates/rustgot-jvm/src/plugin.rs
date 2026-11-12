//! Reads the descriptor (`plugin.yml`) of a Bukkit/Paper plugin jar.

use std::{fs, io::Read, path::Path};

use serde_norway::Value;

/// Descriptors are a few KiB; this only guards against a hostile jar.
const DESCRIPTOR_LIMIT: u64 = 1024 * 1024;

/// The `plugin.yml` fields the bridge needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDescription {
    pub name: String,
    pub version: String,
    pub main: String,
}

/// Reads `plugin.yml` (or `paper-plugin.yml` for Paper-only plugins) from `jar`.
pub fn read_description(jar: &Path) -> Result<PluginDescription, String> {
    let file = fs::File::open(jar).map_err(|err| err.to_string())?;
    let mut zip =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|err| err.to_string())?;
    for descriptor in ["plugin.yml", "paper-plugin.yml"] {
        let Ok(entry) = zip.by_name(descriptor) else {
            continue;
        };
        if entry.size() > DESCRIPTOR_LIMIT {
            return Err(format!(
                "{descriptor} is larger than {DESCRIPTOR_LIMIT} bytes"
            ));
        }
        let mut text = String::new();
        entry
            .take(DESCRIPTOR_LIMIT)
            .read_to_string(&mut text)
            .map_err(|err| format!("couldn't read {descriptor}: {err}"))?;
        return parse_description(&text).map_err(|err| format!("invalid {descriptor}: {err}"));
    }
    Err("no plugin.yml or paper-plugin.yml".to_owned())
}

fn parse_description(yaml: &str) -> Result<PluginDescription, String> {
    let doc: Value = serde_norway::from_str(yaml).map_err(|err| err.to_string())?;
    // Bukkit reads these with `toString()`, so `version: 1.0` is the string "1.0".
    let field = |key: &str| match doc.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        Some(Value::Bool(value)) => Ok(value.to_string()),
        _ => Err(format!("missing `{key}`")),
    };
    Ok(PluginDescription {
        name: field("name")?,
        version: field("version")?,
        main: field("main")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_version_is_read_as_text() {
        let description = parse_description(
            "name: Hello\nversion: 1.0\nmain: org.example.Hello\napi-version: '1.21'\n",
        )
        .unwrap();
        assert_eq!(description.version, "1.0");
        assert_eq!(description.main, "org.example.Hello");
        assert!(parse_description("name: Hello\nversion: 1\n").is_err());
    }
}
