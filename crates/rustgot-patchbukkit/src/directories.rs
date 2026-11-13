use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct PatchBukkitDirectories {
    /// `patchbukkit/`: the bridge config and, optionally, an operator-provided `patchbukkit.jar`.
    pub base: PathBuf,
    pub plugins: PathBuf,
    /// Downloads: the Paper server patched at runtime and the pinned `patchbukkit.jar`.
    pub cache: PathBuf,
    pub paper: PathBuf,
    /// Working directory of the headless Paper server (its configs and shadow level)
    pub paper_runtime: PathBuf,
}

pub fn setup_directories(
    server_dir: &Path,
    plugins_dir_name: &str,
) -> Result<PatchBukkitDirectories, String> {
    let base = server_dir.join("patchbukkit");
    let plugins = if Path::new(plugins_dir_name).is_absolute() {
        PathBuf::from(plugins_dir_name)
    } else {
        server_dir.join(plugins_dir_name)
    };
    let cache = server_dir.join("cache").join("patchbukkit");
    let paper = cache.join("paper");
    let paper_runtime = base.join("paper-runtime");

    for dir in [&base, &plugins, &cache] {
        fs::create_dir_all(dir)
            .map_err(|err| format!("Failed to create {}: {err}", dir.display()))?;
    }

    Ok(PatchBukkitDirectories {
        base,
        plugins,
        cache,
        paper,
        paper_runtime,
    })
}
