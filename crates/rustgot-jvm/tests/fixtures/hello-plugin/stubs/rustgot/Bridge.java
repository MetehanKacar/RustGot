package rustgot;

/** Compile-only stub of the class RustGot defines at runtime (see crates/rustgot-jvm/src/vm.rs). */
public final class Bridge {
    /** Logs through RustGot. Levels: 0 error, 1 warn, 2 info, 3 debug, 4 trace. */
    public static native void log(int level, String source, String message);
}
