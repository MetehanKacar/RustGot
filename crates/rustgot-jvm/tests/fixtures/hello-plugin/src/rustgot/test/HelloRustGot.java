package rustgot.test;

import rustgot.Bridge;

/**
 * Smallest plugin the Phase 3 JVM bridge can run. Build it from this directory with:
 *
 * <pre>
 * javac --release 21 -d out -cp stubs src/rustgot/test/HelloRustGot.java
 * jar --create --file HelloRustGot.jar -C out rustgot/test plugin.yml
 * </pre>
 */
public final class HelloRustGot {
    private HelloRustGot() {}

    public static void onEnable() {
        Bridge.log(2, "HelloRustGot", "Hello from Java " + Runtime.version()
                + " (" + System.getProperty("java.vm.name") + ", max heap "
                + Runtime.getRuntime().maxMemory() / (1024 * 1024) + " MiB)");
    }

    public static void onDisable() {
        Bridge.log(2, "HelloRustGot", "Goodbye from Java");
    }
}
