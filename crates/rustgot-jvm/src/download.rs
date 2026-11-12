//! Downloads `GraalVM` Community JDK from its official GitHub releases into the `RustGot` cache.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::JvmError;

const RELEASES_URL: &str =
    "https://api.github.com/repos/graalvm/graalvm-ce-builds/releases?per_page=30";
const USER_AGENT: &str = concat!("RustGot/", env!("CARGO_PKG_VERSION"));
/// The release list is a few hundred KiB.
const RELEASES_LIMIT: usize = 16 * 1024 * 1024;
/// `GraalVM` archives are about 350 MiB.
const ARCHIVE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const CHECKSUM_LIMIT: usize = 4096;
const PROGRESS_STEP_PERCENT: u64 = 10;

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// File name ending of the `GraalVM` archive for this platform.
fn platform_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("_windows-x64_bin.zip"),
        ("linux", "x86_64") => Some("_linux-x64_bin.tar.gz"),
        ("linux", "aarch64") => Some("_linux-aarch64_bin.tar.gz"),
        ("macos", "aarch64") => Some("_macos-aarch64_bin.tar.gz"),
        _ => None,
    }
}

/// Picks the newest release that has a `GraalVM` Community JDK `feature` archive for this platform
/// together with its `.sha256` file.
///
/// Archive names changed over time (`graalvm-community-jdk-25.0.2_...` for the first JDK 25
/// releases, `graalvm-community-jdk-25i5-25.0.5.0.3_...` for the Innovation releases), so only
/// the JDK feature version prefix and the platform suffix are matched.
fn pick_archive<'a>(
    releases: &'a [Release],
    feature: u32,
    suffix: &str,
) -> Option<(&'a Release, &'a Asset, &'a Asset)> {
    let prefix = format!("graalvm-community-jdk-{feature}");
    releases
        .iter()
        .filter(|release| !release.draft && !release.prerelease)
        .find_map(|release| {
            let archive = release.assets.iter().find(|asset| {
                asset.name.ends_with(suffix)
                    && asset
                        .name
                        .strip_prefix(&prefix)
                        .is_some_and(|rest| rest.starts_with(['.', 'i', '_']))
            })?;
            let checksum_name = format!("{}.sha256", archive.name);
            let checksum = release
                .assets
                .iter()
                .find(|asset| asset.name == checksum_name)?;
            Some((release, archive, checksum))
        })
}

/// Downloads, verifies and extracts `GraalVM` Community JDK `feature` into `cache_dir` and returns
/// the new Java home.
pub async fn install_graalvm(cache_dir: &Path, feature: u32) -> Result<PathBuf, JvmError> {
    let suffix = platform_suffix().ok_or_else(|| {
        JvmError::Download(format!(
            "GraalVM Community does not publish builds for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let client = pumpkin_auth::client::client_builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| JvmError::Download(err.to_string()))?;

    let body = fetch_limited(
        client
            .get(RELEASES_URL)
            .header("Accept", "application/vnd.github+json"),
        RELEASES_LIMIT,
    )
    .await?;
    let releases: Vec<Release> = serde_json::from_slice(&body)
        .map_err(|err| JvmError::Download(format!("unexpected GitHub release list: {err}")))?;
    let (release, archive, checksum) =
        pick_archive(&releases, feature, suffix).ok_or_else(|| {
            JvmError::Download(format!(
                "no GraalVM Community JDK {feature} release for this platform"
            ))
        })?;

    let checksum_text =
        fetch_limited(client.get(&checksum.browser_download_url), CHECKSUM_LIMIT).await?;
    let expected = parse_sha256(&String::from_utf8_lossy(&checksum_text)).ok_or_else(|| {
        JvmError::Download(format!("{} is not a SHA-256 checksum", checksum.name))
    })?;

    tokio::fs::create_dir_all(cache_dir).await?;
    let archive_path = cache_dir.join(&archive.name);
    let partial_path = cache_dir.join(format!("{}.part", archive.name));
    info!(
        "Downloading {} ({}) into {}",
        archive.name,
        release.tag_name,
        cache_dir.display()
    );
    let result = download_verified(
        &client,
        &archive.browser_download_url,
        &partial_path,
        &expected,
    )
    .await;
    if let Err(err) = result {
        let _ = tokio::fs::remove_file(&partial_path).await;
        return Err(err);
    }
    tokio::fs::rename(&partial_path, &archive_path).await?;

    info!("Extracting {}", archive.name);
    let staging = cache_dir.join(format!(".extract-{}", archive.name));
    let cache = cache_dir.to_path_buf();
    let archive_for_task = archive_path.clone();
    let home = tokio::task::spawn_blocking(move || extract(&archive_for_task, &staging, &cache))
        .await
        .map_err(|err| JvmError::Download(format!("extraction task failed: {err}")))??;
    if let Err(err) = tokio::fs::remove_file(&archive_path).await {
        warn!("Couldn't remove {}: {err}", archive_path.display());
    }
    info!("Installed {} in {}", release.tag_name, home.display());
    Ok(home)
}

/// The checksum files hold the bare hex digest; `sha256sum` style `digest  name` is accepted too.
fn parse_sha256(text: &str) -> Option<String> {
    let digest = text.split_whitespace().next()?;
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

async fn fetch_limited(
    request: reqwest::RequestBuilder,
    limit: usize,
) -> Result<Vec<u8>, JvmError> {
    let mut response = request.send().await?.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Err(JvmError::Download(format!(
                "response from {} is larger than {limit} bytes",
                response.url()
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    expected_sha256: &str,
) -> Result<(), JvmError> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length();
    if total.is_some_and(|total| total > ARCHIVE_LIMIT) {
        return Err(JvmError::Download(format!("{url} is too large")));
    }

    let mut file = tokio::fs::File::create(path).await?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut next_report = PROGRESS_STEP_PERCENT;
    while let Some(chunk) = response.chunk().await? {
        downloaded += chunk.len() as u64;
        if downloaded > ARCHIVE_LIMIT {
            return Err(JvmError::Download(format!("{url} is too large")));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        if let Some(total) = total.filter(|&total| total > 0) {
            let percent = downloaded * 100 / total;
            if percent >= next_report {
                info!(
                    "Downloaded {percent}% ({} of {} MiB)",
                    downloaded / (1024 * 1024),
                    total / (1024 * 1024)
                );
                next_report = percent - percent % PROGRESS_STEP_PERCENT + PROGRESS_STEP_PERCENT;
            }
        }
    }
    file.sync_all().await?;
    drop(file);

    let actual = hex::encode(hasher.finalize());
    if actual != expected_sha256 {
        return Err(JvmError::Download(format!(
            "SHA-256 mismatch for {url}: expected {expected_sha256}, got {actual}"
        )));
    }
    info!("Verified SHA-256 {actual}");
    Ok(())
}

/// Extracts `archive` into `staging`, then moves its single top-level JDK directory into
/// `cache_dir`, so an interrupted extraction never looks like an installed runtime.
fn extract(archive: &Path, staging: &Path, cache_dir: &Path) -> Result<PathBuf, JvmError> {
    if staging.exists() {
        fs::remove_dir_all(staging)?;
    }
    fs::create_dir_all(staging)?;

    let file = io::BufReader::new(fs::File::open(archive)?);
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        zip::ZipArchive::new(file)
            .and_then(|mut zip| zip.extract(staging))
            .map_err(|err| JvmError::Download(format!("couldn't extract {name}: {err}")))?;
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.unpack(staging)?;
    }

    let mut roots = fs::read_dir(staging)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("release").is_file());
    let (Some(root), None) = (roots.next(), roots.next()) else {
        return Err(JvmError::Download(format!(
            "{name} does not contain exactly one JDK directory"
        )));
    };
    let file_name = root
        .file_name()
        .ok_or_else(|| JvmError::Download(format!("{name} has an unnamed root")))?;
    let home = cache_dir.join(file_name);
    if home.exists() {
        fs::remove_dir_all(&home)?;
    }
    fs::rename(&root, &home)?;
    fs::remove_dir_all(staging)?;
    Ok(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> Release {
        Release {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease,
            assets: assets
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    browser_download_url: format!("https://example.invalid/{name}"),
                })
                .collect(),
        }
    }

    #[test]
    fn picks_newest_release_with_archive_and_checksum() {
        // Asset names as published on github.com/graalvm/graalvm-ce-builds.
        let releases = [
            release(
                "graal-25.6.0",
                true,
                &[
                    "graalvm-community-jdk-25i6-25.0.6_windows-x64_bin.zip",
                    "graalvm-community-jdk-25i6-25.0.6_windows-x64_bin.zip.sha256",
                ],
            ),
            release(
                "graal-25.5.5.0.3",
                false,
                // Checksum missing: must be skipped.
                &["graalvm-community-jdk-25i5-25.0.5.0.3_windows-x64_bin.zip"],
            ),
            release(
                "graal-25.4.4.1.1",
                false,
                &[
                    "graalvm-community-jdk-25i4-25.0.4.1.1_linux-x64_bin.tar.gz",
                    "graalvm-community-jdk-25i4-25.0.4.1.1_linux-x64_bin.tar.gz.sha256",
                    "graalvm-community-jdk-25i4-25.0.4.1.1_windows-x64_bin.zip",
                    "graalvm-community-jdk-25i4-25.0.4.1.1_windows-x64_bin.zip.sha256",
                ],
            ),
            release(
                "jdk-25.0.2",
                false,
                &[
                    "graalvm-community-jdk-25.0.2_windows-x64_bin.zip",
                    "graalvm-community-jdk-25.0.2_windows-x64_bin.zip.sha256",
                ],
            ),
        ];

        let (release, archive, checksum) =
            pick_archive(&releases, 25, "_windows-x64_bin.zip").unwrap();
        assert_eq!(release.tag_name, "graal-25.4.4.1.1");
        assert_eq!(
            archive.name,
            "graalvm-community-jdk-25i4-25.0.4.1.1_windows-x64_bin.zip"
        );
        assert_eq!(checksum.name, format!("{}.sha256", archive.name));

        let (release, ..) = pick_archive(&releases[3..], 25, "_windows-x64_bin.zip").unwrap();
        assert_eq!(release.tag_name, "jdk-25.0.2");
        assert!(pick_archive(&releases, 2, "_windows-x64_bin.zip").is_none());
        assert!(pick_archive(&releases, 25, "_linux-aarch64_bin.tar.gz").is_none());
    }

    #[test]
    fn parses_bare_and_sha256sum_checksums() {
        let digest = "3a5c4e1b9f0d2e7c8b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c";
        assert_eq!(parse_sha256(digest).as_deref(), Some(digest));
        assert_eq!(
            parse_sha256(&format!("{}  graalvm.zip\n", digest.to_uppercase())).as_deref(),
            Some(digest)
        );
        assert_eq!(parse_sha256("not a digest"), None);
        assert_eq!(parse_sha256(&digest[1..]), None);
    }
}
