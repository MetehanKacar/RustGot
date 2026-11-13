# Java plugin bridge (embedded JVM)

RustGot runs Paper/Spigot/Bukkit plugins (`plugins/*.jar`) in a JVM embedded into the server
process through JNI. Java discovery, download and the base JVM options live in
`crates/rustgot-jvm`; the Paper API comes from PatchBukkit, vendored as
`crates/rustgot-patchbukkit` (Phase 4). Settings are the `jvm:` section of `rustgot.yml`
(`crates/rustgot-config`) plus `patchbukkit/patchbukkit.config.toml` for bridge details.

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

## Phase 4: Paper API through PatchBukkit

### How it is wired

- `crates/rustgot-patchbukkit` is PatchBukkit `d6b55cf` (0.2.2): `src`, `build`, `proto` and the
  Java/Gradle sources in `java/`, with its GPL-3.0 `LICENSE`; the crate `README.md` lists every
  RustGot change. It compiled against our `pumpkin-core` without API fixes.
- `main.rs` registers it with `PluginManager::load_builtin_plugin` (new, small, upstreamable)
  when `plugins/` has jars and `jvm.enabled` is true. `on_load` returns at once; Java, the jar and
  Paper are prepared on the bridge's own runtime, then the JVM boots headless Paper on the
  `RustGot-JVM` thread (16 MiB stack). Players that join earlier get the Bukkit commands when
  the boot finishes (`reload_commands_for_everyone`).
- Java comes from `rustgot_jvm::find_or_install_java` and the JVM is created with
  `JavaVM::with_libjvm` plus `rustgot_jvm::embedding_options` (`-Xrs`, heap from
  `jvm.max-heap-mb`/plugin count, compact headers). `-Dterminal.jline=false` is required: without
  it Paper's log4j console appender probes the terminal through JLine, which on Windows spawns a
  child process that never returns and hangs the boot.
- Shutdown: `PumpkinServer::shutdown` now unloads plugins first (like Paper's `stopServer`),
  then saves and kicks players and saves worlds. The headless server is marked `hasStopped`
  before `disablePlugins`, so `Bukkit.isStopping()` is true (EssentialsX otherwise logs a
  "/reload" error).
- Scheduler: besides `CraftScheduler.mainThreadHeartbeat`, the heartbeat now also ticks Paper's
  global region scheduler, the entity schedulers and the Adventure/dialog click queues, like
  `MinecraftServer.tickChildren`. Plugins using Paper/Folia schedulers (PlaceholderAPI's eCloud,
  FoliaLib users) never ran those tasks before.
- Files: plugins in `plugins/` (Paper also puts their data folders there), bridge config and an
  optional operator-provided `patchbukkit.jar` in `patchbukkit/`, Paper runtime dir in
  `patchbukkit/paper-runtime/`, downloads in `cache/patchbukkit/`.

### Shipping `patchbukkit.jar`

No binaries are committed and the jar is not embedded. `src/java/bridge_jar.rs` looks for
`patchbukkit/patchbukkit.jar`, then `patchbukkit.jar` next to the executable, then downloads
`https://github.com/MetehanKacar/RustGot/releases/download/patchbukkit-0.2.2-rg1/patchbukkit.jar`
and checks the SHA-256 pinned in `RELEASE_SHA256`. **That release does not exist yet**: a
maintainer has to build the jar and upload it under that tag. `java/build-jar.{sh,ps1}` downloads
Gradle 9.8.1 (SHA-256 checked), builds `:patchbukkit:jar` and fails if the hash differs from the
pin (`PATCHBUKKIT_SKIP_PIN=1` / `-SkipPin` to accept a new one when the Java side changes). The
`jar` task is reproducible: two clean builds on this machine (Oracle GraalVM 25.0.4) gave the same
hash; other JDK builds or the moving `paperweight-userdev 2.0.0-SNAPSHOT` may not, which shows up
as a pin mismatch. Building needs a JDK 25 and network (Maven Central, repo.papermc.io).

Paperclip (Paper 26.3 build 205) is downloaded from `fill.papermc.io`; its SHA-256
`a0a58f53…9968` is pinned in `src/java/paper.rs` and must also match what the API reports. A
different configured build falls back to the API's checksum with a warning. Paperclip runs with
the Java `rustgot-jvm` found, so `JAVA_HOME` is not needed.

### Plugin matrix (Windows 11, `--profile iter`, seed `1792752931139676432`, console + loadbot)

Jars from Modrinth, SHA-512 checked against the Modrinth API:
LuckPerms-Bukkit 5.5.71, VaultUnlocked 2.20.3, EssentialsX 2.22.0, PlaceholderAPI 2.12.3,
worldedit-bukkit 7.4.6. Each stage booted with only that plugin and its dependencies. `loadbot`
bots cannot type commands, so commands ran from the console with bots online.

| Plugin | Status | What was checked |
|---|---|---|
| LuckPerms | works | boot, `lp info`, `lp creategroup`, `lp group … permission set/info`, `lp listgroups`; H2 data survived a restart; `lp user bot0 parent add vip` + `permission check essentials.fly` = true for an online bot, with Essentials contexts |
| VaultUnlocked | works | `vault-info` lists LuckPerms as permission and chat provider; EssentialsX finds the Vault economy layer |
| EssentialsX | works (console-checked) | `essentials version`, `balance`/`eco give` (0 → 100), `msg`, `whois` (position, money, IP), `seen`, `list`, join messages; clean disable on stop. Logs "unsupported server version". Teleports, homes, kits, AFK and chat formatting need a real client |
| PlaceholderAPI | works (after the scheduler fix) | `papi version`, `ecloud download Player/Server`, `papi reload`, `papi list`, `papi parse bot0 %player_name%_%player_x%` = `bot0_2056`, `%server_online%` = 2 |
| WorldEdit | partially | loads and enables (all capabilities on Bukkit-Official), `we version`; `//size`, `//undo`, `//redo` reach WorldEdit. Edits could not be checked from the console: `/world`, `/pos1`, `/set`, `/count` printed nothing, and `//cmd args` typed on the console is rejected by Pumpkin's parser. Command names that start with `/` are registered without it and the executor sends the cleaned name, so WorldEdit's `//pos1` may arrive as `pos1`; needs a look at `native_callbacks/command.rs` / `commands/mod.rs` and an in-game check |

Not attempted: FAWE (needs async bulk chunk access from Java) and Multiverse-Core (needs creating
and loading worlds at runtime through Pumpkin); both need new Rust APIs.

### Memory and tick time

Working set (WS) / private bytes in MiB from `Get-Process`, 1 s samples; "idle" is 10-30 s
after "PatchBukkit background initialization complete" (Purpur: after "Done"); "3 bots" is
`rustgot-loadbot` with 3 bots flying 45 s at 0.5 blocks/tick over mostly generated terrain.
Purpur 26.3-2657 ran with `-Xmx2G` and default JVM ergonomics; RustGot's JVM got
`-Xmx576m` (256 + 64 × 5 plugins).

| Setup | idle WS | idle private | 3 bots WS mean (peak) |
|---|---|---|---|
| RustGot, no jars (no JVM) | 33 | 15 | 544 (575) |
| RustGot + LuckPerms | 716 | 738 | - |
| RustGot + LP, Vault, EssentialsX | 877-883 | 918-924 | 1260-1422 (1505) |
| RustGot + all 5 plugins | 881 | 973 | 1441 (1482) |
| same, `-XX:+UseSerialGC -XX:-UseJVMCICompiler -XX:ReservedCodeCacheSize=64m -XX:TieredStopAtLevel=1` | 741 | 884 | 1278 (1310) |
| Purpur, no plugins | 827 | 879 | 1454 (1768) |
| Purpur + all 5 plugins | 1199 | 1247 | 2108 (2281) |

Honest reading: headless Paper costs about 700-850 MiB before any player, so RustGot with Java
plugins idles at the level of Purpur without plugins, and with the same 5 plugins it is only
about 25-35 % below Purpur (881 vs 1199 MiB idle, 1441 vs 2108 MiB with bots). The low-RAM goal
holds only without Java plugins. NMT after boot with 5 plugins: heap 480 MiB committed (274 MiB
live after a full GC), metaspace 146, code cache 63, GC structures 69, classes 43, symbols 36.
The largest live objects are block-state collision shapes (`AABB`, `ArrayVoxelShape`, Moonrise
`CachedShapeData`, about 80 MiB) built by Paper's block bootstrap for a server that never ticks.

Mitigations, in order of payoff:

1. Smaller JVM defaults for the bridge: SerialGC and C1 instead of the Graal JIT saved 140 MiB
   idle and 160 MiB with bots above; a heap cap closer to the live set (for example 384 MiB for
   5 plugins) and `-XX:MaxMetaspaceSize` need testing with more plugins first.
2. Skip headless Paper work nobody uses: block-shape caches, recipe/advancement loading (1866
   advancements are loaded), datapack registries beyond what plugins read, Paper's own Netty and
   Moonrise worker pools.
3. Class-data sharing (an AppCDS archive of the Paper classpath) to share metaspace and cut the
   10-30 s boot.

Tick time (`tick query`, last 100 ticks, 3 bots): without jars P50 9.4-12.2 ms, P95 12.6-17.6,
P99 15.5-25.6. With all 5 plugins P50 8.7-10.5 ms but P95 20-41 ms and P99 30-65 ms (one window
with LP/Vault/EssentialsX had P99 576 ms). Purpur with the same plugins reported 21-43 ms averages
over 5 s windows. The median is unchanged; the tail comes from sync Bukkit events (EssentialsX
listens to `PlayerMoveEvent`) taking the global primary lock on tick and Rayon threads, from Java
GC pauses, and from the bridge contending with the tick. The Paper boot holds the primary lock for
its whole duration, so the first ticks after it were 5.4 s late ("Can't keep up"). Not measured
separately: the cost per hot event; a microbenchmark of `fire_event_direct` for `PlayerMoveEvent`
is the next step.

### Not done / next steps

- Publish `patchbukkit.jar` under the pinned release tag (or ship it next to the binary) and add a
  CI job that builds the jar and the crate.
- Fix command names starting with `/` (WorldEdit) and check WorldEdit in game.
- Hot events: skip move events no listener needs; move the Paper boot off the primary lock.
- `rustgot_jvm::JavaPluginHost` (the Phase 3 proof-of-life bridge) is no longer used by the
  binary; remove it once PatchBukkit is the accepted path.

## Known limits and risks

- Plugins that call `System.exit` end the RustGot process without saving worlds (Paper saves in
  its shutdown hook). Still open with PatchBukkit.
- PatchBukkit stops its JVM worker but does not destroy the JVM; Java shutdown hooks only run
  when the process exits.
- `System.out`/`System.err` of Java code go straight to the console, not through `tracing`.
- On Linux the JVM and Wasmtime both install SIGSEGV/SIGBUS handlers and chain to the previous
  one; this was only tested on Windows.
- GraalVM CE archives are glibc builds; Alpine/musl hosts need a system musl JDK.
- On Windows `jvm.dll` relies on the MSVC runtime (`vcruntime140*.dll`), which RustGot itself
  already needs; a system without the VC++ 2015-2022 redistributable fails to load it.
- A JVM can be created only once per process; there is no Java plugin hot reload.
