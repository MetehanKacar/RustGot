//! Embedded JVM for Paper/Bukkit Java plugins.
//!
//! Nothing here runs unless `plugins/` contains `.jar` files: the JVM is started lazily, so a
//! server without Java plugins pays no memory for it. See `docs/rustgot/jvm-bridge.md`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod discovery;
mod download;
mod plugin;
mod vm;

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use rustgot_config::JvmConfig;
use tracing::{error, info, warn};

pub use discovery::JavaInstall;
pub use vm::embedding_options;

/// Directory, relative to the server directory, that holds Java runtimes `RustGot` downloaded.
const CACHE_DIR: [&str; 2] = ["cache", "jvm"];
/// JDK line downloaded when no suitable Java is installed.
const DOWNLOAD_FEATURE_VERSION: u32 = 25;
const DISABLE_TIMEOUT: Duration = Duration::from_secs(60);
const DESTROY_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors while finding, downloading or starting Java.
#[derive(Debug, thiserror::Error)]
pub enum JvmError {
    #[error("no Java {min} or newer found (set JAVA_HOME, or enable jvm.auto-download)")]
    NoJava { min: u32 },
    #[error("couldn't download Java: {0}")]
    Download(String),
    #[error("couldn't download Java: {0}")]
    Http(#[from] reqwest::Error),
    #[error("couldn't start the JVM: {0}")]
    Start(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The running JVM with the Java plugins it enabled.
pub struct JavaPluginHost {
    vm: vm::Jvm,
}

impl JavaPluginHost {
    /// Finds or downloads Java, starts the JVM and enables `jars`.
    ///
    /// Returns `None`, after logging why, if no plugin could be enabled; the JVM is not kept
    /// running in that case.
    pub async fn start(server_dir: &Path, config: &JvmConfig, jars: Vec<PathBuf>) -> Option<Self> {
        match Self::try_start(server_dir, config, jars).await {
            Ok(host) => host,
            Err(err) => {
                error!("Java plugins will not be loaded: {err}");
                None
            }
        }
    }

    async fn try_start(
        server_dir: &Path,
        config: &JvmConfig,
        jars: Vec<PathBuf>,
    ) -> Result<Option<Self>, JvmError> {
        let plugins = tokio::task::spawn_blocking(move || read_descriptions(jars))
            .await
            .map_err(io::Error::other)?;
        if plugins.is_empty() {
            return Ok(None);
        }

        let install = find_or_install_java(server_dir, config).await?;
        let max_heap_mb = config.max_heap_mb_for(plugins.len());
        info!(
            "Starting Java {} from {} with a {max_heap_mb} MiB heap",
            install.version,
            install.home.display()
        );
        let vm = vm::Jvm::start(install, max_heap_mb).await?;

        let total = plugins.len();
        let enabled = vm.enable_plugins(plugins).await;
        if enabled == 0 {
            warn!("None of the {total} Java plugin(s) could be enabled; stopping the JVM");
            vm.shutdown(DISABLE_TIMEOUT, DESTROY_TIMEOUT).await;
            return Ok(None);
        }
        info!("Enabled {enabled} of {total} Java plugin(s)");
        Ok(Some(Self { vm }))
    }

    /// Disables the Java plugins and stops the JVM.
    pub async fn shutdown(self) {
        self.vm.shutdown(DISABLE_TIMEOUT, DESTROY_TIMEOUT).await;
    }
}

fn read_descriptions(jars: Vec<PathBuf>) -> Vec<vm::PluginJar> {
    jars.into_iter()
        .filter_map(|path| match plugin::read_description(&path) {
            Ok(description) => Some(vm::PluginJar { path, description }),
            Err(err) => {
                error!("Couldn't load {}: {err}", path.display());
                None
            }
        })
        .collect()
}

/// Finds a suitable Java, or downloads `GraalVM` Community JDK into `<server_dir>/cache/jvm` when
/// `jvm.auto-download` allows it.
pub async fn find_or_install_java(
    server_dir: &Path,
    config: &JvmConfig,
) -> Result<JavaInstall, JvmError> {
    let cache_dir = &server_dir.join(CACHE_DIR.iter().collect::<PathBuf>());
    let min = config.min_java_version;
    let search_dir = cache_dir.clone();
    let found = tokio::task::spawn_blocking(move || discovery::find_java(min, &search_dir))
        .await
        .map_err(io::Error::other)?;
    if let Some(install) = found {
        return Ok(install);
    }
    if !config.auto_download {
        return Err(JvmError::NoJava { min });
    }

    let feature = min.max(DOWNLOAD_FEATURE_VERSION);
    info!(
        "No Java {min} or newer found; downloading GraalVM Community JDK {feature} (about 350 MiB, only once)"
    );
    let home = download::install_graalvm(cache_dir, feature).await?;
    tokio::task::spawn_blocking(move || discovery::inspect(&home, None))
        .await
        .map_err(io::Error::other)?
        .map_err(|reason| JvmError::Download(format!("the downloaded JDK is unusable: {reason}")))
}
