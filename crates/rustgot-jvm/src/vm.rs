//! Owns the embedded JVM on a dedicated thread.
//!
//! The thread that creates the JVM becomes its main thread. Every JNI call goes through it, so
//! Java plugins see one consistent "server thread" and Tokio workers never block on Java code.

use std::{ffi::c_void, path::PathBuf, sync::mpsc, thread, time::Duration};

use jni::{
    Env, EnvUnowned, InitArgsBuilder, JavaVM, NativeMethod,
    errors::{Error, ThrowRuntimeExAndDefault},
    jni_sig, jni_str,
    objects::{JClass, JClassLoader, JObject, JString, JValue},
    refs::Global,
    strings::JNIStr,
    sys::jint,
};
use tokio::sync::oneshot;
use tracing::{debug, error, info, trace, warn};

use crate::{JvmError, discovery::JavaInstall, plugin::PluginDescription};

const THREAD_NAME: &str = "RustGot-JVM";
const THREAD_STACK_BYTES: usize = 4 * 1024 * 1024;
/// A small initial heap keeps the young generation, and with it the resident memory, small until
/// plugins actually need more.
const INITIAL_HEAP_MB: u32 = 32;

/// Internal name of the class `RustGot` defines for plugins to call into Rust.
const BRIDGE_CLASS: &JNIStr = jni_str!("rustgot/Bridge");
const LOG_NAME: &JNIStr = jni_str!("log");
const LOG_DESCRIPTOR: &JNIStr = jni_str!("(ILjava/lang/String;Ljava/lang/String;)V");

/// A Java plugin jar with its parsed descriptor.
pub struct PluginJar {
    pub path: PathBuf,
    pub description: PluginDescription,
}

struct EnabledPlugin {
    description: PluginDescription,
    main: Global<JClass<'static>>,
    loader: Global<JObject<'static>>,
}

enum Command {
    EnablePlugins {
        jars: Vec<PluginJar>,
        reply: oneshot::Sender<usize>,
    },
    Shutdown {
        disabled: oneshot::Sender<()>,
        destroyed: oneshot::Sender<()>,
    },
}

/// Handle to the JVM thread.
pub struct Jvm {
    commands: mpsc::Sender<Command>,
}

impl Jvm {
    /// Starts the JVM from `install` with a heap limit of `max_heap_mb`.
    pub async fn start(install: JavaInstall, max_heap_mb: u32) -> Result<Self, JvmError> {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .stack_size(THREAD_STACK_BYTES)
            .spawn(move || run(&install, max_heap_mb, ready_tx, &receiver))?;
        ready_rx
            .await
            .map_err(|_| JvmError::Start("the JVM thread exited during startup".to_owned()))??;
        Ok(Self { commands })
    }

    /// Enables `jars` in order and returns how many plugins are enabled now.
    pub async fn enable_plugins(&self, jars: Vec<PluginJar>) -> usize {
        let (reply, enabled) = oneshot::channel();
        if self
            .commands
            .send(Command::EnablePlugins { jars, reply })
            .is_err()
        {
            return 0;
        }
        enabled.await.unwrap_or(0)
    }

    /// Disables every plugin, then destroys the JVM, which runs Java shutdown hooks after all
    /// non-daemon Java threads have ended.
    pub async fn shutdown(self, disable_timeout: Duration, destroy_timeout: Duration) {
        let (disabled_tx, disabled) = oneshot::channel();
        let (destroyed_tx, destroyed) = oneshot::channel();
        if self
            .commands
            .send(Command::Shutdown {
                disabled: disabled_tx,
                destroyed: destroyed_tx,
            })
            .is_err()
        {
            return;
        }
        if tokio::time::timeout(disable_timeout, disabled)
            .await
            .is_err()
        {
            warn!(
                "Java plugins did not finish disabling within {}s; stopping without them",
                disable_timeout.as_secs()
            );
            return;
        }
        if tokio::time::timeout(destroy_timeout, destroyed)
            .await
            .is_err()
        {
            warn!(
                "Java threads are still running {}s after the plugins were disabled; stopping without waiting for them",
                destroy_timeout.as_secs()
            );
            return;
        }
        info!("Stopped the JVM");
    }
}

fn run(
    install: &JavaInstall,
    max_heap_mb: u32,
    ready: oneshot::Sender<Result<(), JvmError>>,
    commands: &mpsc::Receiver<Command>,
) {
    let vm = match create_vm(install, max_heap_mb) {
        Ok(vm) => vm,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut plugins = Vec::new();
    while let Ok(command) = commands.recv() {
        match command {
            Command::EnablePlugins { jars, reply } => {
                plugins.extend(jars.into_iter().filter_map(|jar| enable(&vm, jar)));
                let _ = reply.send(plugins.len());
            }
            Command::Shutdown {
                disabled,
                destroyed,
            } => {
                // Reverse order, so a plugin is disabled before the plugins it depends on.
                for plugin in std::mem::take(&mut plugins).into_iter().rev() {
                    disable(&vm, plugin);
                }
                let _ = disabled.send(());
                // SAFETY: every global reference was dropped with its plugin above, no Java frame
                // is on this thread's stack, and nothing uses the VM afterwards because this
                // thread returns right after. HotSpot makes the jni crate's detach on thread exit
                // a no-op once the VM is destroyed.
                if let Err(err) = unsafe { vm.destroy() } {
                    warn!("Couldn't destroy the JVM: {err}");
                }
                let _ = destroyed.send(());
                return;
            }
        }
    }
}

fn create_vm(install: &JavaInstall, max_heap_mb: u32) -> Result<JavaVM, JvmError> {
    let mut args = InitArgsBuilder::new()
        // Leave SIGINT, SIGTERM and the Windows console handler to RustGot, so Ctrl+C still
        // stops the server cleanly instead of exiting from inside the JVM.
        .option("-Xrs")
        .option(format!("-Xmx{max_heap_mb}m"))
        .option(format!("-Xms{}m", max_heap_mb.min(INITIAL_HEAP_MB)));
    if install.feature >= 25 {
        args = args.option("-XX:+UseCompactObjectHeaders");
    }
    let args = args
        .build()
        .map_err(|err| JvmError::Start(err.to_string()))?;
    let vm = JavaVM::with_libjvm(args, || Ok(&install.libjvm))
        .map_err(|err| JvmError::Start(err.to_string()))?;
    vm.attach_current_thread(define_bridge)
        .map_err(|err| JvmError::Start(format!("couldn't define {BRIDGE_CLASS}: {err}")))?;
    Ok(vm)
}

/// Class file of
/// `public final class rustgot.Bridge { public static native void log(int level, String source, String message); }`.
///
/// It is assembled here instead of compiled by `javac`, so building `RustGot` needs no JDK. The
/// levels are 0 error, 1 warn, 2 info, 3 debug and 4 trace.
fn bridge_class_file() -> Vec<u8> {
    fn u16_be(class: &mut Vec<u8>, value: u16) {
        class.extend_from_slice(&value.to_be_bytes());
    }
    // Class files store names in modified UTF-8, which is what `JNIStr` holds.
    fn utf8(class: &mut Vec<u8>, text: &JNIStr) {
        const CONSTANT_UTF8: u8 = 1;
        let bytes = text.to_bytes();
        class.push(CONSTANT_UTF8);
        u16_be(class, bytes.len() as u16);
        class.extend_from_slice(bytes);
    }
    const CONSTANT_CLASS: u8 = 7;
    const ACC_PUBLIC_FINAL_SUPER: u16 = 0x0031;
    const ACC_PUBLIC_STATIC_NATIVE: u16 = 0x0109;
    // Java 8, so any runtime the config allows can load it.
    const CLASS_FILE_MAJOR_VERSION: u16 = 52;

    let mut class = 0xCAFE_BABEu32.to_be_bytes().to_vec();
    u16_be(&mut class, 0);
    u16_be(&mut class, CLASS_FILE_MAJOR_VERSION);
    // Constant pool: #1 this class, #2 its name, #3 Object, #4 its name, #5 and #6 the method.
    u16_be(&mut class, 7);
    class.push(CONSTANT_CLASS);
    u16_be(&mut class, 2);
    utf8(&mut class, BRIDGE_CLASS);
    class.push(CONSTANT_CLASS);
    u16_be(&mut class, 4);
    utf8(&mut class, jni_str!("java/lang/Object"));
    utf8(&mut class, LOG_NAME);
    utf8(&mut class, LOG_DESCRIPTOR);
    u16_be(&mut class, ACC_PUBLIC_FINAL_SUPER);
    u16_be(&mut class, 1); // this_class
    u16_be(&mut class, 3); // super_class
    u16_be(&mut class, 0); // interfaces
    u16_be(&mut class, 0); // fields
    u16_be(&mut class, 1); // methods
    u16_be(&mut class, ACC_PUBLIC_STATIC_NATIVE);
    u16_be(&mut class, 5);
    u16_be(&mut class, 6);
    u16_be(&mut class, 0); // method attributes
    u16_be(&mut class, 0); // class attributes
    class
}

/// `rustgot.Bridge.log(int, String, String)`.
extern "system" fn log<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    level: jint,
    source: JString<'local>,
    message: JString<'local>,
) {
    env.with_env(|env| -> Result<(), Error> {
        let source = source.try_to_string(env)?;
        let message = message.try_to_string(env)?;
        match level {
            0 => error!("[{source}] {message}"),
            1 => warn!("[{source}] {message}"),
            3 => debug!("[{source}] {message}"),
            4 => trace!("[{source}] {message}"),
            _ => info!("[{source}] {message}"),
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

fn define_bridge(env: &mut Env<'_>) -> Result<(), Error> {
    let loader = JClassLoader::get_system_class_loader(env)?;
    let class = env.define_class(Some(BRIDGE_CLASS), &loader, &bridge_class_file())?;
    // SAFETY: `log` is a static native method in the class file above, and its Rust signature
    // (env, class, int, String, String) matches `LOG_DESCRIPTOR`.
    let log_method =
        unsafe { NativeMethod::from_raw_parts(LOG_NAME, LOG_DESCRIPTOR, log as *mut c_void) };
    // SAFETY: as above, the descriptor and the static-ness match the function.
    unsafe { env.register_native_methods(&class, &[log_method]) }
}

fn enable(vm: &JavaVM, jar: PluginJar) -> Option<EnabledPlugin> {
    let PluginJar { path, description } = jar;
    info!("Enabling {} v{}", description.name, description.version);
    let loaded = vm.attach_current_thread(|env| -> Result<_, Error> {
        let path = env.new_string(path.to_string_lossy())?;
        let file = env.new_object(
            jni_str!("java/io/File"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&path)],
        )?;
        let uri = env
            .call_method(&file, jni_str!("toURI"), jni_sig!("()Ljava/net/URI;"), &[])?
            .l()?;
        let url = env
            .call_method(&uri, jni_str!("toURL"), jni_sig!("()Ljava/net/URL;"), &[])?
            .l()?;
        let urls = env.new_object_array(1, jni_str!("java/net/URL"), &url)?;
        let name = env.new_string(&description.name)?;
        let parent = JClassLoader::get_system_class_loader(env)?;
        let loader = env.new_object(
            jni_str!("java/net/URLClassLoader"),
            jni_sig!("(Ljava/lang/String;[Ljava/net/URL;Ljava/lang/ClassLoader;)V"),
            &[
                JValue::Object(&name),
                JValue::Object(&urls),
                JValue::Object(&parent),
            ],
        )?;
        let main = env.new_string(&description.main)?;
        let main = env
            .call_static_method(
                jni_str!("java/lang/Class"),
                jni_str!("forName"),
                jni_sig!("(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;"),
                &[
                    JValue::Object(&main),
                    JValue::Bool(true),
                    JValue::Object(&loader),
                ],
            )?
            .l()?;
        let main = env.cast_local::<JClass>(main)?;
        env.call_static_method(&main, jni_str!("onEnable"), jni_sig!("()V"), &[])?;
        Ok((env.new_global_ref(main)?, env.new_global_ref(loader)?))
    });

    match loaded {
        Ok((main, loader)) => Some(EnabledPlugin {
            description,
            main,
            loader,
        }),
        Err(Error::CaughtJavaException { name, msg, .. })
            if name == "java.lang.NoClassDefFoundError" && is_server_api_class(&msg) =>
        {
            warn!(
                "{} v{} needs the Bukkit/Paper API ({msg}), which the Java bridge does not provide yet; skipping it",
                description.name, description.version
            );
            None
        }
        Err(err) => {
            error!(
                "Couldn't enable {} v{}: {err}",
                description.name, description.version
            );
            None
        }
    }
}

/// Whether `class` (an internal name from `NoClassDefFoundError`) belongs to the server API.
fn is_server_api_class(class: &str) -> bool {
    [
        "org/bukkit/",
        "io/papermc/",
        "com/destroystokyo/paper/",
        "org/spigotmc/",
    ]
    .iter()
    .any(|package| class.starts_with(package))
}

fn disable(vm: &JavaVM, plugin: EnabledPlugin) {
    let EnabledPlugin {
        description,
        main,
        loader,
    } = plugin;
    info!("Disabling {} v{}", description.name, description.version);
    let result = vm.attach_current_thread(|env| -> Result<(), Error> {
        match env.call_static_method(&main, jni_str!("onDisable"), jni_sig!("()V"), &[]) {
            Ok(_) | Err(Error::MethodNotFound { .. }) => {}
            Err(err) => return Err(err),
        }
        // Releases the jar, which Windows keeps locked while it is open.
        env.call_method(&loader, jni_str!("close"), jni_sig!("()V"), &[])?;
        Ok(())
    });
    if let Err(err) = result {
        error!(
            "Error while disabling {} v{}: {err}",
            description.name, description.version
        );
    }
}
