# Java plugin bridge (embedded JVM)

RustGot runs Paper/Spigot/Bukkit plugins (`plugins/*.jar`) in a JVM embedded into the server
process through JNI. Code lives in `crates/rustgot-jvm`; settings are the `jvm:` section of
`rustgot.yml` (`crates/rustgot-config`).

## Phase 3 (done): lazy JVM foundation

- **Zero cost without jars.** `main.rs` only calls `JavaPluginHost::start` when `plugins/`
  contains `.jar` files and `jvm.enabled` is true. Without jars no JVM library is loaded and no
  thread is started. Jars without a readable `plugin.yml`/`paper-plugin.yml` are skipped before
  the JVM starts; if no plugin can be enabled the JVM is destroyed again right away.
- **Java discovery** (`discovery.rs`), first match wins:
  1. `JAVA_HOME` (version from its `release` file),
  2. `java` on `PATH` (`java -XshowSettings:properties -version` gives `java.home` and
     `java.version`, because `PATH` entries are often launcher stubs or symlinks),
  3. well-known install dirs (`%ProgramFiles%\{Java,Eclipse Adoptium,Microsoft,Zulu,...}\*`,
     `/usr/lib/jvm/*`, `/usr/lib64/jvm/*`, `/usr/java/*`, `/opt/java/*`),
  4. runtimes RustGot downloaded into `cache/jvm/`.

  A candidate must have feature version >= `jvm.min-java-version` (default 25), a matching
  `OS_ARCH` and a `server` (or `client`) `libjvm`.
- **Auto-download** (`download.rs`), unless `jvm.auto-download: false`: the newest non-prerelease
  release of https://github.com/graalvm/graalvm-ce-builds that has a
  `graalvm-community-jdk-<N>…_<os>-<arch>_bin.{zip,tar.gz}` asset plus its `.sha256` file
  (N = max(25, min-java-version)). Platforms: windows-x64, linux-x64, linux-aarch64. The archive
  is streamed to `cache/jvm/<name>.part` with SHA-256 computed on the fly, verified, extracted into
  a staging dir on the blocking pool and then renamed into `cache/jvm/<jdk dir>`, so an interrupted
  download/extraction is never picked up. Progress is logged every 10%. Network I/O is async;
  hashing is per chunk; extraction uses `spawn_blocking`.
- **JVM start** (`vm.rs`): `jni` 0.22 with the `invocation` feature; `JavaVM::with_libjvm` loads the
  discovered `libjvm` at runtime, so building RustGot needs no JDK. The JVM lives on its own OS
  thread `RustGot-JVM` (4 MiB stack); it becomes the JVM main thread and every JNI call is sent
  to it over a channel, so Tokio workers never run Java code. Options:
  - `-Xrs`: the JVM must not install SIGINT/SIGTERM/console handlers, otherwise Ctrl+C would
    exit the process from inside the JVM without saving worlds;
  - `-Xmx` = `jvm.max-heap-mb`, or automatically 256 MiB + 64 MiB per plugin, capped at 2 GiB;
  - `-Xms32m` (or less), so the young generation and resident memory start small;
  - `-XX:+UseCompactObjectHeaders` on Java 25+.
- **Shutdown**: after `PumpkinServer::start` returns, plugins are disabled in reverse order
  (60 s limit), their class loaders closed, then `DestroyJavaVM` runs Java shutdown hooks once all
  non-daemon Java threads have ended (10 s limit; after that the process exits anyway).
- **Proof-of-life bridge**: RustGot defines one class at runtime,
  `rustgot.Bridge` with `public static native void log(int level, String source, String message)`
  (class file assembled in `vm.rs`, so no `javac` at build time). Each plugin jar gets its own
  `URLClassLoader`; the `main` class from `plugin.yml` is initialized and its static `onEnable()` /
  `onDisable()` are called. Real Bukkit plugins fail with `NoClassDefFoundError: org/bukkit/...`;
  that is reported as "needs the Bukkit/Paper API, which the Java bridge does not provide yet".
  The fixture is `crates/rustgot-jvm/tests/fixtures/hello-plugin` (build commands in its source).

### Measurements (Windows 11, `--profile iter`, new world, idle 10 s after "Server is now running")

| Setup | Working set | Private bytes | Threads |
|---|---|---|---|
| no jars (JVM never loaded) | 31-32 MiB | 13-14 MiB | 86 |
| `HelloRustGot.jar`, Oracle GraalVM 25.0.4, default flags | 82 MiB | 155 MiB | 102 |
| same, `-XX:-UseJVMCICompiler` (C2 instead of Graal JIT) | 62 MiB | 130 MiB | 101 |
| same, `-XX:+UseSerialGC` | 82 MiB | 96 MiB | 97 |
| same, SerialGC + C2 + `-XX:ReservedCodeCacheSize=64m` | 61 MiB | 67 MiB | 96 |

The extra flags were tried through `JAVA_TOOL_OPTIONS` only; defaults stay with JVM ergonomics
until real plugins (Phase 4) show what GC pauses and JIT choice cost on the tick.

GraalVM CE download path: 330 MiB archive in about 43 s, SHA-256 verified, extraction about 24 s;
the next start finds it in `cache/jvm/` with no network access.

## PatchBukkit evaluation

Repository: https://github.com/Pumpkin-MC/PatchBukkit (Pumpkin org, GPL-3.0, active: release
0.2.2 on 2026-11-11 targeting Pumpkin `0.2.2+26.3-26.60`, the version this fork is based on;
25 open issues). License is compatible with RustGot (GPL-3.0).

How it works (master `d6b55cf`):

- It is a Pumpkin **native plugin** (`cdylib`, `#[plugin_impl]`) of ~11k lines of Rust plus ~11k
  lines of Java (`patchbukkit.jar`, built with Gradle + paperweight-userdev, embedded with
  `rust-embed`) and protobuf definitions (`proto/`).
- It does not reimplement the Bukkit API any more. On first start it downloads Paperclip for a
  pinned Paper build (26.3 build 205) from `fill.papermc.io`, verifies SHA-256, patches it locally
  and boots a **real headless Paper server** (`HeadlessPaperServer`: full NMS registries,
  `DedicatedServer`, `CraftServer`, never ticked) inside the JVM. ByteBuddy (as a `-javaagent`)
  instruments NMS/CraftBukkit (`WorldBridge`, `EntityBridge`, `PlayerBridge`, `InventoryBridge`,
  `CommandBridge`, `SchedulerBridge`, …) so reads/writes go to Pumpkin.
- Rust → Java: JNI calls (`HeadlessPaperServer.boot`, event firing, commands, tab complete).
  Java → Rust: Panama FFM upcalls, protobuf-encoded (`NativeBridgeFfi`, generated by a custom
  `protoc-gen-ffi`) plus primitive "fast path" calls for hot operations (block get/set, teleports,
  packets).
- Events: one Pumpkin event handler per Pumpkin event type (`events/handler.rs`, ~4.5k lines)
  converts to protobuf, fires the Bukkit event, applies cancellation/modifications back.
  Sync Bukkit events run directly on the calling Rust thread (tick thread, Rayon worker or Tokio
  worker) under a re-entrant global "primary" lock; async Bukkit events run without it.
- Main thread: Paper's `TickThread.isTickThread` is patched; `isPrimaryThread` is true for the
  JVM worker, `Server-Ticker` and any thread holding the primary lock. The Bukkit scheduler
  heartbeat (`CraftScheduler.mainThreadHeartbeat`) runs on Pumpkin's `ServerTickStartEvent`.
- Plugin loading/dependency order is Paper's own `PaperPluginManagerImpl`; plugins live in
  `patchbukkit/patchbukkit-plugins/`, not `plugins/`.
- Integration tests in `rust/tests/plugins/` drive EssentialsX, LuckPerms, VaultUnlocked,
  PlaceholderAPI, WorldEdit, WorldGuard, Multiverse-Core, Chunky, GrimAC, PacketEvents,
  ProtocolLib, ViaVersion, TAB, GSit, AuraSkills, Veinminer with simulated players.

What it needs from the Rust side: the Pumpkin native plugin `Context` (event registration for
~150 event types, command registration and `reload_commands_for_everyone`, data folder), direct
access to `Server`, `World` (block state get/set with `BlockFlags`, chunks, weather/time,
entities, item drops, particles, sounds), `Player` (teleport, inventory, messages, abilities,
packets in/out), `pumpkin-data` registries and `pumpkin-protocol` packets.

Findings that matter for RustGot:

1. **It cannot be loaded as a prebuilt `.dll/.so` into RustGot.** Native Pumpkin plugins share
   Rust types (`PluginMetadata`, `Context`, `Server`) across the dylib boundary with no stable
   ABI; only `PLUGIN_API_VERSION` is checked. A binary built against upstream Pumpkin crashes or
   corrupts memory on a fork with different struct layouts (see PatchBukkit issue #17, an OOM in
   `PluginMetadata::clone` inside `NativePluginLoader::load`). It has to be compiled into the
   RustGot binary from source, against our `pumpkin-core`.
2. **Memory.** A headless Paper with full NMS registries is a large fixed cost (a real Paper
   server object graph, datapack registries, Paper's libraries). That conflicts with RustGot's
   low-RAM goal and needs measurement before adopting it as the default; it is still far less than
   running Paper/Purpur itself, because worlds, chunks and entities stay in Rust.
3. **Compatibility is much better than a Bukkit-API reimplementation**: since CraftBukkit/NMS are
   real, plugins that touch NMS/CraftBukkit internals partly work too. The project decision was
   "NMS/CraftBukkit plugins unsupported"; with PatchBukkit they are "best effort, unsupported".
4. It downloads Mojang/Paper code at runtime only (Paperclip), which keeps redistribution clean.

## Phase 4 plan: Paper API through PatchBukkit

1. **Vendor PatchBukkit's Rust side as `crates/rustgot-patchbukkit`** (GPL-3.0, keep its history
   or a pinned upstream commit; `git subtree` makes re-syncing practical). Replace its own JVM
   worker start (`JavaVM::new` + `java-locator`) with `rustgot-jvm` (discovery, GraalVM download,
   `-Xrs`, heap policy, lazy start). Keep plugins in `plugins/`.
2. **Register it in-process** instead of through `NativePluginLoader`: give `PluginManager` a way
   to add a built-in `Arc<dyn Plugin>` (small upstreamable change), so PatchBukkit's
   `on_load`/`on_unload` run at Pumpkin's plugin points. Disable Java plugins **before** players
   are kicked and worlds saved (Paper disables plugins first in `stopServer`); today's Phase 3
   hook runs after `PumpkinServer::start` returns, which is too late once plugins touch players.
3. **Ship `patchbukkit.jar`** without a build-time JDK: build it in RustGot CI (Gradle +
   paperweight) and either place it next to the binary in release archives or download it on first
   Java-plugin start from our GitHub release with a SHA-256 pinned in the source, like GraalVM.
4. **API drift**: compile PatchBukkit against our `pumpkin-core`, fix call sites, and keep a CI
   job that builds it so upstream merges that break the bridge are noticed.
5. **Threading review** before enabling by default: sync Bukkit events currently run on whichever
   thread fires them under a global lock, including Rayon workers in the tick; measure MSPT impact
   with plugins listening to hot events (move, block physics).
6. **Measure memory**: RSS of RustGot with 0, 1 and 6 plugins, Paper headless vs. Purpur.
7. **Plugin matrix**, one stage per row, each with a scripted bot/console check plus an in-game
   check:

   | Plugin | What to verify | Main bridge surface |
   |---|---|---|
   | LuckPerms | `/lp user … permission set`, permission checks seen by other plugins, storage (H2/SQLite) survives restart | permissions, commands, services, async tasks |
   | Vault (VaultUnlocked) | economy/permission/chat providers registered and found | `ServicesManager` |
   | EssentialsX | `/spawn`, `/home`, `/tpa`, `/msg`, `/balance`, `/eco`, kits, AFK, join/quit messages | teleports, inventories, chat, player data, scheduler |
   | PlaceholderAPI | `/papi parse me %player_name%`, expansions download | chat, plugin messaging, HTTP |
   | WorldEdit (then FAWE) | `//wand`, `//set`, `//undo`, schematic load/save; FAWE adds async chunk access | bulk block get/set, chunk access, NBT, selections |
   | Multiverse-Core | create/load/teleport between worlds | world creation/loading through Pumpkin, per-world settings |

   FAWE and Multiverse need new Rust APIs (async bulk chunk edits, creating worlds at runtime);
   schedule them last.

## Known limits and risks

- Plugins that call `System.exit` end the RustGot process without saving worlds (Paper saves in
  its shutdown hook). Needs an `exit` hook or a `Runtime` patch in Phase 4.
- `System.out`/`System.err` of Java code go straight to the console, not through `tracing`.
- On Linux the JVM and Wasmtime both install SIGSEGV/SIGBUS handlers and chain to the previous
  one; this was only tested on Windows.
- GraalVM CE archives are glibc builds; Alpine/musl hosts need a system musl JDK.
- On Windows `jvm.dll` relies on the MSVC runtime (`vcruntime140*.dll`), which RustGot itself
  already needs; a system without the VC++ 2015-2022 redistributable fails to load it.
- A JVM can be created only once per process; there is no Java plugin hot reload.
