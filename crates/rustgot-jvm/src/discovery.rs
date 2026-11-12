//! Finds an installed Java runtime whose `libjvm` can be embedded.

use std::{
    collections::HashSet,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use tracing::{debug, info};

/// A Java installation that can be embedded through its `libjvm`.
#[derive(Debug, Clone)]
pub struct JavaInstall {
    /// The `java.home` directory.
    pub home: PathBuf,
    /// Full version string, like `25.0.4`.
    pub version: String,
    /// Feature (major) version, like `25`.
    pub feature: u32,
    /// Path of `jvm.dll`, `libjvm.so` or `libjvm.dylib`.
    pub libjvm: PathBuf,
}

/// The `release` file of a JDK is a few KiB; anything far larger is not one.
const RELEASE_FILE_LIMIT: u64 = 256 * 1024;

const JAVA_EXE: &str = if cfg!(windows) { "java.exe" } else { "java" };

/// Returns the first suitable Java in priority order: `JAVA_HOME`, `java` on `PATH`, well-known
/// install directories, then runtimes `RustGot` downloaded into `cache_dir`.
pub fn find_java(min_feature: u32, cache_dir: &Path) -> Option<JavaInstall> {
    let mut seen = HashSet::new();
    let mut consider = |source: &str, home: PathBuf, version_hint: Option<String>| {
        let key = fs::canonicalize(&home).unwrap_or_else(|_| home.clone());
        if !seen.insert(key) {
            return None;
        }
        match inspect(&home, version_hint) {
            Ok(install) if install.feature >= min_feature => Some(install),
            Ok(install) => {
                info!(
                    "Ignoring Java {} from {source} at {}: Java plugins need Java {min_feature} or newer",
                    install.version,
                    home.display()
                );
                None
            }
            Err(reason) => {
                debug!(
                    "Ignoring Java from {source} at {}: {reason}",
                    home.display()
                );
                None
            }
        }
    };

    if let Some(home) = env::var_os("JAVA_HOME").filter(|home| !home.is_empty())
        && let Some(install) = consider("JAVA_HOME", PathBuf::from(home), None)
    {
        return Some(install);
    }

    if let Some(java) = java_on_path() {
        match probe_java_binary(&java) {
            Ok((home, version)) => {
                if let Some(install) = consider("PATH", home, version) {
                    return Some(install);
                }
            }
            Err(reason) => debug!("Ignoring {}: {reason}", java.display()),
        }
    }

    for home in well_known_homes() {
        if let Some(install) = consider("the system", home, None) {
            return Some(install);
        }
    }

    for home in subdirectories_newest_first(cache_dir) {
        if let Some(install) = consider("the RustGot cache", home, None) {
            return Some(install);
        }
    }

    None
}

/// Reads the version of the Java at `home` from its `release` file, falling back to
/// `version_hint` (from `java -XshowSettings`) for runtimes without one.
pub fn inspect(home: &Path, version_hint: Option<String>) -> Result<JavaInstall, String> {
    let release = read_release_file(home);
    let version = release
        .as_deref()
        .and_then(|release| release_value(release, "JAVA_VERSION"))
        .map(str::to_owned)
        .or(version_hint)
        .ok_or("no `release` file with JAVA_VERSION")?;
    let feature =
        feature_version(&version).ok_or_else(|| format!("unrecognized version {version:?}"))?;

    if let Some(arch) = release
        .as_deref()
        .and_then(|release| release_value(release, "OS_ARCH"))
        && !arch_matches(arch)
    {
        return Err(format!(
            "built for {arch}, but RustGot runs on {}",
            env::consts::ARCH
        ));
    }

    let libjvm = libjvm_path(home).ok_or("no server or client libjvm")?;
    Ok(JavaInstall {
        home: home.to_path_buf(),
        version,
        feature,
        libjvm,
    })
}

fn read_release_file(home: &Path) -> Option<String> {
    let file = fs::File::open(home.join("release")).ok()?;
    let mut content = String::new();
    file.take(RELEASE_FILE_LIMIT)
        .read_to_string(&mut content)
        .ok()?;
    Some(content)
}

/// Value of `KEY="value"` in a JDK `release` file.
fn release_value<'a>(release: &'a str, key: &str) -> Option<&'a str> {
    release.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix('=')?;
        Some(value.trim().trim_matches('"'))
    })
}

/// Feature version of a `java.version` string: `25.0.4` is 25, `1.8.0_392` is 8, `26-ea` is 26.
fn feature_version(version: &str) -> Option<u32> {
    let mut numbers = version.split(|c: char| !c.is_ascii_digit());
    let first: u32 = numbers.next()?.parse().ok()?;
    if first == 1 {
        numbers.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Whether a `release` file `OS_ARCH` can be loaded into this process.
fn arch_matches(os_arch: &str) -> bool {
    match env::consts::ARCH {
        "x86_64" => matches!(os_arch, "x86_64" | "amd64"),
        "aarch64" => matches!(os_arch, "aarch64" | "arm64"),
        "x86" => matches!(os_arch, "x86" | "i386" | "i586" | "i686"),
        arch => os_arch == arch,
    }
}

fn libjvm_path(home: &Path) -> Option<PathBuf> {
    let (dir, file) = if cfg!(windows) {
        ("bin", "jvm.dll")
    } else if cfg!(target_os = "macos") {
        ("lib", "libjvm.dylib")
    } else {
        ("lib", "libjvm.so")
    };
    ["server", "client"]
        .into_iter()
        .map(|vm| home.join(dir).join(vm).join(file))
        .find(|path| path.is_file())
}

fn java_on_path() -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(JAVA_EXE))
        .find(|path| path.is_file())
}

/// Runs `java -XshowSettings:properties -version` to learn `java.home` and `java.version`.
///
/// `java` on `PATH` is often a launcher stub or a symlink chain (`javapath`, `alternatives`),
/// so asking the runtime itself is the only reliable way to find its home.
fn probe_java_binary(java: &Path) -> Result<(PathBuf, Option<String>), String> {
    let output = Command::new(java)
        .args(["-XshowSettings:properties", "-version"])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run it: {err}"))?;
    // The settings are printed to stderr.
    let text = String::from_utf8_lossy(&output.stderr);
    let home = settings_property(&text, "java.home").ok_or("it did not report java.home")?;
    let version = settings_property(&text, "java.version").map(str::to_owned);
    Ok((PathBuf::from(home), version))
}

/// Value of `    key = value` in `-XshowSettings:properties` output.
fn settings_property<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (name, value) = line.trim_start().split_once(" = ")?;
        (name == key).then(|| value.trim())
    })
}

fn well_known_homes() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Some(dir) = env::var_os(var) {
                let dir = PathBuf::from(dir);
                for vendor in [
                    "Java",
                    "Eclipse Adoptium",
                    "Microsoft",
                    "Zulu",
                    "BellSoft",
                    "Amazon Corretto",
                    "Semeru",
                    "GraalVM",
                ] {
                    roots.push(dir.join(vendor));
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        return subdirectories_newest_first(Path::new("/Library/Java/JavaVirtualMachines"))
            .into_iter()
            .map(|bundle| bundle.join("Contents").join("Home"))
            .collect();
    } else {
        for dir in ["/usr/lib/jvm", "/usr/lib64/jvm", "/usr/java", "/opt/java"] {
            roots.push(PathBuf::from(dir));
        }
    }
    roots
        .iter()
        .flat_map(|root| subdirectories_newest_first(root))
        .collect()
}

/// Subdirectories sorted by name, descending, so `jdk-25.0.5` comes before `jdk-25.0.2`.
fn subdirectories_newest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort_by(|a, b| b.cmp(a));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_version_handles_legacy_and_suffixed_versions() {
        assert_eq!(feature_version("25.0.4"), Some(25));
        assert_eq!(feature_version("25"), Some(25));
        assert_eq!(feature_version("26-ea"), Some(26));
        assert_eq!(feature_version("1.8.0_392"), Some(8));
        assert_eq!(feature_version(""), None);
        assert_eq!(feature_version("ea"), None);
    }

    #[test]
    fn release_value_reads_quoted_values() {
        let release = "IMPLEMENTOR=\"GraalVM Community\"\r\nJAVA_VERSION=\"25.0.2\"\r\n\
                       JAVA_VERSION_DATE=\"2026-01-20\"\r\nOS_ARCH=\"x86_64\"\r\n";
        assert_eq!(release_value(release, "JAVA_VERSION"), Some("25.0.2"));
        assert_eq!(release_value(release, "OS_ARCH"), Some("x86_64"));
        assert_eq!(release_value(release, "MODULES"), None);
    }

    #[test]
    fn settings_property_reads_show_settings_output() {
        // Trimmed output of `java -XshowSettings:properties -version` on Windows.
        let output = "Property settings:\r\n    file.encoding = UTF-8\r\n    java.class.path = \r\n    \
                      java.home = C:\\GraalVM\\graalvm-jdk-25.0.4+7.1\r\n    java.library.path = C:\\a\r\n        \
                      C:\\b\r\n    java.version = 25.0.4\r\n    java.version.date = 2026-07-21\r\n\r\n\
                      java version \"25.0.4\" 2026-07-21 LTS\r\n";
        assert_eq!(
            settings_property(output, "java.home"),
            Some("C:\\GraalVM\\graalvm-jdk-25.0.4+7.1")
        );
        assert_eq!(settings_property(output, "java.version"), Some("25.0.4"));
        assert_eq!(settings_property(output, "java.class.path"), Some(""));
    }
}
