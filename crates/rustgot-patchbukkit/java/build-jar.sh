#!/usr/bin/env sh
# Builds patchbukkit.jar from these sources and checks it against the SHA-256 pinned in
# ../src/java/bridge_jar.rs. Needs a JDK 25 (JAVA_HOME) and network access to Maven Central,
# repo.papermc.io and services.gradle.org. Output: patchbukkit/build/libs/patchbukkit.jar
# Set PATCHBUKKIT_SKIP_PIN=1 to accept a different hash (when updating the pin).
set -eu

GRADLE_VERSION=9.8.1
GRADLE_SHA256=dce76f55f8e251a3a1f130eb120f30b3d271de2b76c9b0729d316b5a1b6dc01f

here=$(cd "$(dirname "$0")" && pwd)
cache=${GRADLE_USER_HOME:-$HOME/.gradle}
dist="$cache/rustgot/gradle-$GRADLE_VERSION"

if [ ! -x "$dist/bin/gradle" ]; then
    mkdir -p "$cache/rustgot"
    zip="$cache/rustgot/gradle-$GRADLE_VERSION-bin.zip"
    curl -fsSL -o "$zip" "https://services.gradle.org/distributions/gradle-$GRADLE_VERSION-bin.zip"
    echo "$GRADLE_SHA256  $zip" | sha256sum -c -
    unzip -q -o "$zip" -d "$cache/rustgot"
    rm "$zip"
fi

"$dist/bin/gradle" -p "$here" --no-daemon --console=plain :patchbukkit:jar

jar="$here/patchbukkit/build/libs/patchbukkit.jar"
pinned=$(sed -n 's/^pub const RELEASE_SHA256: &str = "\([0-9a-f]*\)";/\1/p' "$here/../src/java/bridge_jar.rs")
actual=$(sha256sum "$jar" | cut -d' ' -f1)
echo "$actual  $jar"
if [ "$actual" != "$pinned" ] && [ "${PATCHBUKKIT_SKIP_PIN:-0}" != 1 ]; then
    echo "SHA-256 differs from the pinned $pinned" >&2
    exit 1
fi
