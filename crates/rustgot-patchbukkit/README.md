# rustgot-patchbukkit

Paper/Bukkit plugin support for RustGot. This crate is a modified copy of
[PatchBukkit](https://github.com/Pumpkin-MC/PatchBukkit) by the PumpkinMC contributors,
licensed under the GNU General Public License v3.0 (see `LICENSE`). Upstream design notes are in
`ARCHITECTURE.md`.

Upstream base: commit `d6b55cf` (release 0.2.2, targeting Pumpkin `0.2.2+26.3-26.60`).
Copied: `rust/src` → `src`, `rust/build` → `build`, `proto`, `java` (without the Gradle wrapper jar).

RustGot changes (2026-11):

- Compiled into the server and registered with `PluginManager::load_builtin_plugin` instead of
  being loaded as a native-plugin `cdylib` (`#[plugin_impl]` removed).
- Java discovery, GraalVM download and the base JVM options (`-Xrs`, heap, compact headers) come
  from `rustgot-jvm`; the JVM is created with `JavaVM::with_libjvm`.
- `patchbukkit.jar` is no longer embedded with `rust-embed`; see `src/java/bridge_jar.rs` and
  `java/build-jar.{sh,ps1}`.
- Paperclip is downloaded with `reqwest` and checked against a SHA-256 pinned in
  `src/java/paper.rs`, then run with the Java `rustgot-jvm` found.
- Plugins are read from `plugins/`; bridge files live in `patchbukkit/` and `cache/patchbukkit/`.
- The `jar` task is reproducible so the pinned jar hash can be rebuilt from source.
