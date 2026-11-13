//! Finding `patchbukkit.jar`, the Java half of the bridge.
//!
//! RustGot does not embed it in the binary (upstream PatchBukkit does, through `rust-embed`), so
//! building RustGot needs no JDK. It is built from `crates/rustgot-patchbukkit/java` by
//! `java/build-jar.{sh,ps1}`, which checks the result against [`RELEASE_SHA256`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const JAR_NAME: &str = "patchbukkit.jar";
/// Tag of the RustGot release that carries the jar built from the vendored Java sources.
pub const RELEASE_TAG: &str = "patchbukkit-0.2.2-rg1";
/// SHA-256 of that jar; the build script refuses to produce a jar with a different hash.
pub const RELEASE_SHA256: &str = "41685cb814315835ab0fb6da51c141d4904597e88894c270339f748bd9a1730d";
const RELEASE_URL: &str = "https://github.com/MetehanKacar/RustGot/releases/download";
const MAX_JAR_BYTES: u64 = 128 * 1024 * 1024;

/// Returns the jar to use, downloading the pinned release into `cache_dir` when the operator
/// placed none in `bridge_dir` or next to the executable.
pub async fn ensure_bridge_jar(bridge_dir: &Path, cache_dir: &Path) -> Result<PathBuf> {
    let mut candidates = vec![bridge_dir.join(JAR_NAME)];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        candidates.push(exe_dir.join(JAR_NAME));
    }
    for candidate in candidates {
        if candidate.is_file() {
            tracing::info!("Using {}", candidate.display());
            return Ok(candidate);
        }
    }

    let cached = cache_dir.join(format!("{RELEASE_TAG}.jar"));
    if sha256_of(&cached)
        .await
        .is_some_and(|hash| hash == RELEASE_SHA256)
    {
        return Ok(cached);
    }
    download(&cached).await.with_context(|| {
        format!(
            "{JAR_NAME} was not found in {} or next to the executable, and the pinned release could not be downloaded (build it with crates/rustgot-patchbukkit/java/build-jar.sh or .ps1)",
            bridge_dir.display()
        )
    })?;
    Ok(cached)
}

async fn download(dest: &Path) -> Result<()> {
    let url = format!("{RELEASE_URL}/{RELEASE_TAG}/{JAR_NAME}");
    tracing::info!("Downloading {url}");
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("RustGot-PatchBukkit/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)?;
    let tmp = dest.with_extension("part");
    let mut out = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    while let Some(chunk) = response.chunk().await? {
        total += chunk.len() as u64;
        if total > MAX_JAR_BYTES {
            drop(out);
            let _ = tokio::fs::remove_file(&tmp).await;
            bail!("{url} is larger than {MAX_JAR_BYTES} bytes");
        }
        hasher.update(&chunk);
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    drop(out);
    let actual = hex::encode(hasher.finalize());
    if actual != RELEASE_SHA256 {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("SHA-256 mismatch for {url}: expected {RELEASE_SHA256}, got {actual}");
    }
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

async fn sha256_of(path: &Path) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(hex::encode(Sha256::digest(&bytes)))
}
