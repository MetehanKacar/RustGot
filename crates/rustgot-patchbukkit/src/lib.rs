//! Paper/Bukkit plugin support for RustGot.
//!
//! Vendored from PatchBukkit (<https://github.com/Pumpkin-MC/PatchBukkit>, GPL-3.0, upstream
//! commit `d6b55cf`, release 0.2.2). Upstream builds it as a Pumpkin native plugin `cdylib`;
//! RustGot compiles it into the server and registers it with
//! `PluginManager::load_builtin_plugin`, because native plugins have no stable ABI. Java is found
//! or downloaded by `rustgot-jvm`. See `docs/rustgot/jvm-bridge.md`.
#![allow(clippy::async_yields_async)]

use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

use pumpkin::plugin::{Context, Plugin, PluginFuture, PluginMetadata};

pub mod commands;
pub mod config;
pub mod directories;
pub mod events;
pub mod java;
pub mod proto;

use directories::setup_directories;
use tokio::sync::{mpsc, oneshot};

use crate::{
    config::patchbukkit::PatchBukkitConfig,
    java::{
        bridge_jar::ensure_bridge_jar,
        jvm::{commands::JvmCommand, worker::JvmWorker},
        paper::ensure_paper_server,
    },
};

/// Name the bridge is registered under in Pumpkin's plugin manager.
pub const PLUGIN_NAME: &str = "PatchBukkit";
/// Stack of the thread that becomes the JVM main thread and boots Paper.
const JVM_THREAD_STACK: usize = 16 * 1024 * 1024;

#[must_use]
pub fn metadata() -> PluginMetadata {
    PluginMetadata {
        name: PLUGIN_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        authors: vec!["PumpkinMC Contributors".to_owned()],
        description: "Runs Paper/Bukkit plugins in an embedded headless Paper server".to_owned(),
        dependencies: Vec::new(),
        permissions: Vec::new(),
    }
}

/// Everything the bridge needs before the JVM can start: Java, `patchbukkit.jar` and the patched
/// Paper server. Downloads happen here, on the bridge's runtime.
async fn prepare_jvm(
    server_dir: PathBuf,
    jvm_config: rustgot_config::JvmConfig,
    plugin_count: usize,
    dirs: &directories::PatchBukkitDirectories,
    config: &PatchBukkitConfig,
) -> anyhow::Result<(rustgot_jvm::JavaInstall, Vec<String>, PathBuf, Vec<PathBuf>)> {
    let java = rustgot_jvm::find_or_install_java(&server_dir, &jvm_config).await?;
    let max_heap_mb = jvm_config.max_heap_mb_for(plugin_count);
    let base_options = rustgot_jvm::embedding_options(&java, max_heap_mb);
    tracing::info!(
        "Java plugin bridge: Java {} ({}), {max_heap_mb} MiB heap",
        java.version,
        java.home.display()
    );
    let jar = ensure_bridge_jar(&dirs.base, &dirs.cache).await?;
    let paper = ensure_paper_server(&dirs.paper, &config.paper, &java.home).await?;
    tracing::info!(
        "Using Paper {} build {} ({} classpath entries)",
        config.paper.version,
        config.paper.build,
        paper.len()
    );
    Ok((java, base_options, jar, paper))
}

pub async fn on_load_inner(plugin: &PatchBukkitPlugin, server: Arc<Context>) -> Result<(), String> {
    tracing::info!("Starting PatchBukkit");

    let base = plugin.server_dir.join("patchbukkit");
    std::fs::create_dir_all(&base)
        .map_err(|e| format!("Failed to create PatchBukkit base directory: {e}"))?;

    let config = PatchBukkitConfig::get_or_create(base.join("patchbukkit.config.toml"))
        .map_err(|e| format!("Failed to setup PatchBukkit config: {e}"))?;

    let dirs = setup_directories(&plugin.server_dir, &config.plugins.directory)?;

    let runtime_handle = plugin.runtime.handle().clone();
    let command_tx = plugin.command_tx.clone();
    let server_clone = server.clone();
    let prepared = {
        let (server_dir, jvm_config, plugin_count) = (
            plugin.server_dir.clone(),
            plugin.jvm_config.clone(),
            plugin.plugin_count,
        );
        let config = config.clone();
        async move {
            prepare_jvm(server_dir, jvm_config, plugin_count, &dirs, &config)
                .await
                .map(|prepared| (prepared, dirs))
        }
    };

    // Run JVM initialization and Paper's native plugin bootstrap in a background task
    // on PatchBukkit's dedicated runtime so Pumpkin's main startup is never blocked.
    plugin.runtime.spawn(async move {
        tracing::info!("Initializing JVM and loading plugins via Paper in background...");

        let ((java, base_options, patchbukkit_jar, paper_classpath), dirs) = match prepared.await {
            Ok(prepared) => prepared,
            Err(e) => {
                tracing::error!("Java plugins will not be loaded: {e:#}");
                return;
            }
        };

        let (tx, rx) = oneshot::channel();
        if let Err(e) = command_tx
            .send(JvmCommand::Initialize {
                java: Box::new(java),
                base_options,
                patchbukkit_jar,
                paper_classpath,
                paper_runtime_path: dirs.paper_runtime.clone(),
                plugins_dir: dirs.plugins.clone(),
                respond_to: tx,
                context: server_clone.clone(),
                runtime_handle,
                command_tx: command_tx.clone(),
                config: Box::new(config),
            })
            .await
        {
            tracing::error!("Failed to send command to initialize JVM: {e}");
            return;
        }
        match rx.await {
            Ok(Ok(())) => {
                crate::events::handler::JVM_READY.store(true, Ordering::Release);

                crate::java::native_callbacks::events::register_single_event::<
                    pumpkin::plugin::server::packet::PacketReceivedEvent,
                >(
                    &server_clone,
                    &command_tx,
                    pumpkin::plugin::EventPriority::Normal,
                    false,
                );

                crate::java::native_callbacks::events::register_single_event::<
                    pumpkin::plugin::server::packet::PacketSentEvent,
                >(
                    &server_clone,
                    &command_tx,
                    pumpkin::plugin::EventPriority::Normal,
                    true,
                );

                crate::java::native_callbacks::events::register_single_event::<
                    pumpkin::plugin::server::server_tick_start::ServerTickStartEvent,
                >(
                    &server_clone,
                    &command_tx,
                    pumpkin::plugin::EventPriority::Normal,
                    false,
                );

                // Players that joined before the Java plugins finished loading were sent the
                // command tree without any Bukkit commands. Resend it so they get commands and
                // suggestions without having to rejoin.
                server_clone.reload_commands_for_everyone();

                tracing::info!("PatchBukkit background initialization complete");
            }
            Ok(Err(e)) => tracing::error!("Failed to initialize JVM and plugins: {e}"),
            Err(e) => tracing::error!("Unable to receive response from JVM initialization: {e}"),
        }
    });

    tracing::info!("PatchBukkit loaded successfully");
    Ok(())
}

pub async fn on_unload_inner(
    plugin: &PatchBukkitPlugin,
    _server: Arc<Context>,
) -> Result<(), String> {
    crate::events::handler::JVM_READY.store(false, Ordering::Release);
    {
        let (tx, rx) = oneshot::channel();
        // The JVM may never have started (no Java, a failed download); shut the worker down anyway.
        let disabled = match plugin
            .command_tx
            .send(JvmCommand::DisableAllPlugins { respond_to: tx })
            .await
        {
            Ok(()) => rx
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string())),
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = disabled {
            tracing::warn!("Java plugins were not disabled: {e}");
        }
    }

    {
        let (tx, rx) = oneshot::channel();
        plugin
            .command_tx
            .send(JvmCommand::Shutdown { respond_to: tx })
            .await
            .map_err(|e| format!("Failed to send command to shutdown: {e}"))?;
        rx.await
            .map_err(|e| format!("Unable to receive response from shutdown: {e}"))?
            .map_err(|e| format!("Failed to shutdown: {e}"))?;
    }

    Ok(())
}

/// The bridge, registered as a built-in Pumpkin plugin.
pub struct PatchBukkitPlugin {
    pub command_tx: mpsc::Sender<JvmCommand>,
    /// Leaked on purpose: Java threads keep its handle, and dropping a runtime from async code
    /// (where Pumpkin drops unloaded plugins) panics.
    pub runtime: &'static tokio::runtime::Runtime,
    server_dir: PathBuf,
    jvm_config: rustgot_config::JvmConfig,
    plugin_count: usize,
}

impl PatchBukkitPlugin {
    /// Creates the bridge's runtime and JVM thread; the JVM itself starts in `on_load`.
    ///
    /// `plugin_count` is the number of plugin jars, used to size the heap.
    pub fn new(
        server_dir: PathBuf,
        jvm_config: rustgot_config::JvmConfig,
        plugin_count: usize,
    ) -> std::io::Result<Self> {
        let runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_name("patchbukkit-worker")
                .enable_all()
                .build()?,
        ));

        let (tx, rx) = mpsc::channel(4096);

        std::thread::Builder::new()
            .name("RustGot-JVM".to_string())
            .stack_size(JVM_THREAD_STACK)
            .spawn(move || {
                JvmWorker::new(rx).attach_thread();
            })?;
        Ok(Self {
            command_tx: tx,
            runtime,
            server_dir,
            jvm_config,
            plugin_count,
        })
    }
}

impl Plugin for PatchBukkitPlugin {
    fn on_load(&self, server: Arc<Context>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(on_load_inner(self, server))
    }

    fn on_unload(&self, server: Arc<Context>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(on_unload_inner(self, server))
    }
}
