//! `duragent upgrade` — self-update the binary from GitHub Releases.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use duragent::build_info;
use duragent::client::AgentClient;
use duragent::config::Config;

const GITHUB_REPO: &str = "giosakti/duragent";

// ============================================================================
// GitHub API Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

// ============================================================================
// Output Types
// ============================================================================

#[derive(Debug, Serialize)]
struct UpgradeResult {
    current_version: String,
    target_version: String,
    update_available: bool,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

// ============================================================================
// Entry Point
// ============================================================================

pub async fn run(
    check_only: bool,
    target_version: Option<String>,
    restart: bool,
    config_path: &str,
    port: Option<u16>,
    format: &str,
) -> Result<()> {
    let current = parse_version(build_info::VERSION)?;
    let release = fetch_release(target_version.as_deref()).await?;
    let release_version = parse_release_version(&release.tag_name)?;

    if current == release_version {
        let result = UpgradeResult {
            current_version: current.to_string(),
            target_version: release_version.to_string(),
            update_available: false,
            action: "up_to_date".to_string(),
            target: None,
        };
        render(&result, format)?;
        return Ok(());
    }

    if check_only {
        let result = UpgradeResult {
            current_version: current.to_string(),
            target_version: release_version.to_string(),
            update_available: true,
            action: "check".to_string(),
            target: None,
        };
        render(&result, format)?;
        if format == "text" {
            println!("\nRun `duragent upgrade` to update.");
        }
        return Ok(());
    }

    // Download and install
    let target_triple = detect_target()?;
    download_and_install(&release, target_triple, format).await?;

    let action = if release_version > current {
        "upgraded"
    } else {
        "downgraded"
    };
    let result = UpgradeResult {
        current_version: current.to_string(),
        target_version: release_version.to_string(),
        update_available: true,
        action: action.to_string(),
        target: Some(target_triple.to_string()),
    };
    render(&result, format)?;

    if restart {
        restart_server(config_path, port).await?;
    }

    Ok(())
}

// ============================================================================
// Version Helpers
// ============================================================================

fn parse_version(v: &str) -> Result<semver::Version> {
    semver::Version::parse(v).with_context(|| format!("Invalid version: {v}"))
}

fn parse_release_version(tag: &str) -> Result<semver::Version> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(stripped)
}

fn normalize_version_tag(v: &str) -> String {
    if v.starts_with('v') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

// ============================================================================
// Platform Detection
// ============================================================================

fn detect_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => bail!(
            "No pre-built binary for {os}/{arch}. \
             Build from source: cargo install --git https://github.com/{GITHUB_REPO}.git"
        ),
    }
}

fn asset_name(target: &str) -> String {
    format!("duragent-{target}.tar.gz")
}

fn find_asset<'a>(release: &'a GitHubRelease, target: &str) -> Result<&'a GitHubAsset> {
    let name = asset_name(target);
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .with_context(|| {
            format!(
                "No asset '{}' found in release {}. Available: {}",
                name,
                release.tag_name,
                release
                    .assets
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        })
}

// ============================================================================
// GitHub API
// ============================================================================

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("duragent/{}", build_info::VERSION))
        .build()
        .context("Failed to build HTTP client")
}

async fn fetch_release(target_version: Option<&str>) -> Result<GitHubRelease> {
    let client = http_client()?;
    let url = match target_version {
        Some(v) => {
            let tag = normalize_version_tag(v);
            format!("https://api.github.com/repos/{GITHUB_REPO}/releases/tags/{tag}")
        }
        None => format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
    };

    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to query GitHub Releases API")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        match target_version {
            Some(v) => bail!("Release '{}' not found", normalize_version_tag(v)),
            None => bail!("No releases found for {GITHUB_REPO}"),
        }
    }

    if !response.status().is_success() {
        bail!(
            "GitHub API returned HTTP {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }

    response
        .json()
        .await
        .context("Failed to parse GitHub release response")
}

// ============================================================================
// Download and Install
// ============================================================================

async fn download_and_install(release: &GitHubRelease, target: &str, format: &str) -> Result<()> {
    let asset = find_asset(release, target)?;
    let current_exe = std::env::current_exe().context("Failed to determine current binary path")?;
    let binary_dir = current_exe
        .parent()
        .context("Failed to determine binary directory")?;
    let archive_path = binary_dir.join("duragent.upgrade.tar.gz");
    let tmp_binary = binary_dir.join("duragent.upgrade.tmp");

    if format == "text" {
        println!(
            "Downloading duragent {} for {}...",
            release.tag_name, target
        );
    }

    // Download archive
    download_file(&asset.browser_download_url, &archive_path).await?;

    // Verify checksum if available
    verify_checksum_if_available(release, &archive_path, format).await?;

    // Extract binary from tar.gz
    extract_binary(&archive_path, &tmp_binary)?;

    // Clean up archive
    let _ = std::fs::remove_file(&archive_path);

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_binary, std::fs::Permissions::from_mode(0o755))
            .context("Failed to set executable permissions")?;
    }

    // Test the new binary
    if format == "text" {
        println!("Verifying new binary...");
    }
    test_binary(&tmp_binary)?;

    // Atomic replace
    if format == "text" {
        println!("Replacing binary at {}...", current_exe.display());
    }
    std::fs::rename(&tmp_binary, &current_exe).with_context(|| {
        // Clean up tmp on failure
        let _ = std::fs::remove_file(&tmp_binary);
        format!(
            "Failed to replace binary at {}. Permission denied? Try: sudo duragent upgrade",
            current_exe.display()
        )
    })?;

    Ok(())
}

async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let client = http_client()?;
    let response = client.get(url).send().await.context("Download failed")?;

    if !response.status().is_success() {
        bail!("Download failed: HTTP {}", response.status());
    }

    let bytes = response.bytes().await.context("Failed to read download")?;
    std::fs::write(dest, &bytes).with_context(|| format!("Failed to write {}", dest.display()))?;
    Ok(())
}

async fn download_text(url: &str) -> Result<String> {
    let client = http_client()?;
    let response = client.get(url).send().await.context("Download failed")?;

    if !response.status().is_success() {
        bail!("Download failed: HTTP {}", response.status());
    }

    response.text().await.context("Failed to read response")
}

// ============================================================================
// Checksum Verification
// ============================================================================

async fn verify_checksum_if_available(
    release: &GitHubRelease,
    archive_path: &Path,
    format: &str,
) -> Result<()> {
    let checksum_asset = release.assets.iter().find(|a| a.name == "checksums.sha256");

    let Some(checksum_asset) = checksum_asset else {
        if format == "text" {
            println!("No checksums.sha256 in release, skipping verification.");
        }
        return Ok(());
    };

    if format == "text" {
        print!("Verifying checksum... ");
    }

    let archive_name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("duragent.upgrade.tar.gz");

    match verify_checksum(
        &checksum_asset.browser_download_url,
        archive_path,
        archive_name,
    )
    .await
    {
        Ok(()) => {
            if format == "text" {
                println!("OK");
            }
            Ok(())
        }
        Err(e) => {
            if format == "text" {
                println!("FAILED");
            }
            Err(e)
        }
    }
}

async fn verify_checksum(
    checksum_url: &str,
    archive_path: &Path,
    archive_filename: &str,
) -> Result<()> {
    let checksum_content = download_text(checksum_url).await?;
    let expected = parse_checksum(&checksum_content, archive_filename)?;
    let actual = compute_sha256(archive_path)?;

    if actual != expected {
        bail!(
            "Checksum mismatch: expected {expected}, got {actual}. \
             The download may be corrupted or tampered with."
        );
    }
    Ok(())
}

fn compute_sha256(path: &Path) -> Result<String> {
    let data = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let hash = Sha256::digest(&data);
    Ok(format!("{:x}", hash))
}

fn parse_checksum(content: &str, filename: &str) -> Result<String> {
    for line in content.lines() {
        // sha256sum format: "hash  filename" (two spaces) or "hash filename"
        let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
        if parts.len() == 2 {
            let file = parts[1].trim();
            if file == filename {
                return Ok(parts[0].to_string());
            }
        }
    }
    bail!("No checksum found for '{filename}' in checksums file")
}

// ============================================================================
// Archive Extraction
// ============================================================================

fn extract_binary(archive_path: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .context("Failed to read archive entries")?
    {
        let mut entry = entry.context("Failed to read archive entry")?;
        let path = entry.path().context("Failed to read entry path")?;

        if path.file_name().and_then(|n| n.to_str()) == Some("duragent") {
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .context("Failed to extract binary from archive")?;
            std::fs::write(dest, &data).with_context(|| {
                format!("Failed to write extracted binary to {}", dest.display())
            })?;
            return Ok(());
        }
    }

    bail!("Archive does not contain a 'duragent' binary")
}

// ============================================================================
// Binary Verification
// ============================================================================

fn test_binary(path: &Path) -> Result<()> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("Failed to execute {}", path.display()))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(path);
        bail!(
            "New binary failed --version check (exit code: {}). Download may be corrupt.",
            output.status
        );
    }
    Ok(())
}

// ============================================================================
// Server Restart
// ============================================================================

async fn restart_server(config_path: &str, port_override: Option<u16>) -> Result<()> {
    let config = Config::load(config_path).await?;
    let port = port_override.unwrap_or(config.server.port);

    let base_url = format!("http://127.0.0.1:{port}");
    let client = AgentClient::new(&base_url);

    // Check if server is running
    if client.health().await.is_err() {
        println!("No running server found, skipping restart.");
        return Ok(());
    }

    println!("Restarting server...");

    // Shut down the running server
    shutdown_server(&base_url, config.server.admin_token.as_deref()).await?;

    // Wait for the server to fully stop
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if client.health().await.is_err() {
            break;
        }
    }

    // exec() into new binary with serve args
    exec_serve(config_path, port)?;

    Ok(())
}

async fn shutdown_server(base_url: &str, admin_token: Option<&str>) -> Result<()> {
    let url = format!("{base_url}/api/admin/v1/shutdown");
    let client = reqwest::Client::new();
    let mut req = client.post(&url);
    if let Some(token) = admin_token {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = req
        .send()
        .await
        .context("Failed to send shutdown request")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("Failed to shut down server: HTTP {status} {body}")
}

#[cfg(unix)]
fn exec_serve(config_path: &str, port: u16) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("Failed to determine binary path")?;
    let err = std::process::Command::new(exe)
        .args([
            "serve",
            "--config",
            config_path,
            "--port",
            &port.to_string(),
        ])
        .exec();

    // exec() only returns on error
    Err(err).context("Failed to exec new binary")
}

#[cfg(not(unix))]
fn exec_serve(_config_path: &str, _port: u16) -> Result<()> {
    bail!("--restart is only supported on Unix")
}

// ============================================================================
// Rendering
// ============================================================================

fn render(result: &UpgradeResult, format: &str) -> Result<()> {
    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(result)?);
        }
        _ => render_text(result),
    }
    Ok(())
}

fn render_text(result: &UpgradeResult) {
    match result.action.as_str() {
        "up_to_date" => {
            println!("Current: v{} (latest)", result.current_version);
            println!("Already up to date.");
        }
        "check" => {
            println!("Current: v{}", result.current_version);
            println!("Latest:  v{}", result.target_version);
        }
        "upgraded" => {
            println!("Upgraded to v{}", result.target_version);
        }
        "downgraded" => {
            println!("Downgraded to v{}", result.target_version);
        }
        _ => {}
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_target_returns_valid_triple() {
        // Should succeed on supported CI/dev platforms
        let result = detect_target();
        if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            assert_eq!(result.unwrap(), "x86_64-unknown-linux-gnu");
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
            assert_eq!(result.unwrap(), "x86_64-apple-darwin");
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            assert_eq!(result.unwrap(), "aarch64-apple-darwin");
        }
    }

    #[test]
    fn normalize_version_tag_adds_prefix() {
        assert_eq!(normalize_version_tag("0.5.0"), "v0.5.0");
    }

    #[test]
    fn normalize_version_tag_keeps_existing_prefix() {
        assert_eq!(normalize_version_tag("v0.5.0"), "v0.5.0");
    }

    #[test]
    fn parse_release_version_strips_prefix() {
        let v = parse_release_version("v0.6.0").unwrap();
        assert_eq!(v, semver::Version::new(0, 6, 0));
    }

    #[test]
    fn parse_release_version_without_prefix() {
        let v = parse_release_version("0.6.0").unwrap();
        assert_eq!(v, semver::Version::new(0, 6, 0));
    }

    #[test]
    fn parse_checksum_finds_matching_file() {
        let content =
            "abc123  other-file.tar.gz\ndef456  duragent-x86_64-unknown-linux-gnu.tar.gz\n";
        let result = parse_checksum(content, "duragent-x86_64-unknown-linux-gnu.tar.gz").unwrap();
        assert_eq!(result, "def456");
    }

    #[test]
    fn parse_checksum_missing_file_errors() {
        let content = "abc123  other-file.tar.gz\n";
        let result = parse_checksum(content, "duragent-x86_64-unknown-linux-gnu.tar.gz");
        assert!(result.is_err());
    }

    #[test]
    fn find_asset_matches_target() {
        let release = GitHubRelease {
            tag_name: "v0.6.0".to_string(),
            assets: vec![
                GitHubAsset {
                    name: "duragent-x86_64-unknown-linux-gnu.tar.gz".to_string(),
                    browser_download_url: "https://example.com/linux.tar.gz".to_string(),
                },
                GitHubAsset {
                    name: "duragent-aarch64-apple-darwin.tar.gz".to_string(),
                    browser_download_url: "https://example.com/mac-arm.tar.gz".to_string(),
                },
            ],
        };
        let asset = find_asset(&release, "x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(asset.name, "duragent-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn find_asset_missing_target_errors() {
        let release = GitHubRelease {
            tag_name: "v0.6.0".to_string(),
            assets: vec![GitHubAsset {
                name: "duragent-x86_64-unknown-linux-gnu.tar.gz".to_string(),
                browser_download_url: "https://example.com/linux.tar.gz".to_string(),
            }],
        };
        let result = find_asset(&release, "aarch64-apple-darwin");
        assert!(result.is_err());
    }

    #[test]
    fn version_comparison_works() {
        let current = parse_version("0.5.0").unwrap();
        let newer = parse_release_version("v0.6.0").unwrap();
        let older = parse_release_version("v0.4.0").unwrap();
        let same = parse_release_version("v0.5.0").unwrap();

        assert!(newer > current);
        assert!(older < current);
        assert_eq!(same, current);
    }
}
