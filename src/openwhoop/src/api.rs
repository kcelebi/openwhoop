use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://api.prod.whoop.com";

/// Upper bound on base64-decoded firmware payload (mitigates memory pressure from hostile API JSON).
const MAX_FIRMWARE_ZIP_BYTES: usize = 512 * 1024 * 1024;
/// Mitigates zip bombs with huge central directories.
const MAX_ZIP_ENTRIES: usize = 10_000;
/// Per-entry uncompressed size cap (firmware partitions are far smaller).
const MAX_UNCOMPRESSED_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
/// Total uncompressed bytes written under `output_dir`.
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .https_only(true)
        .build()
        .context("failed to build HTTP client")
}

#[derive(Serialize)]
struct SignInRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct SignInResponse {
    access_token: String,
    #[allow(dead_code)]
    access_token_expires_in: Option<u64>,
}

#[derive(Serialize)]
struct FirmwareRequest {
    current_chip_firmwares: Vec<ChipFirmware>,
    chip_firmwares_of_upgrade: Vec<ChipFirmware>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ChipFirmware {
    pub chip_name: String,
    pub version: String,
}

#[derive(Deserialize)]
struct FirmwareResponse {
    firmware_zip_file: Option<String>,
    firmware_file: Option<String>,
    desired_device_firmware_config: Option<DeviceFirmwareConfig>,
}

#[derive(Deserialize)]
struct DeviceFirmwareConfig {
    hardware_device: Option<String>,
    chip_firmwares: Option<Vec<ChipFirmwareInfo>>,
    force_update: Option<bool>,
}

#[derive(Deserialize)]
struct ChipFirmwareInfo {
    chip_name: String,
    version: String,
}

pub struct WhoopApiClient {
    client: reqwest::Client,
    token: String,
}

impl WhoopApiClient {
    pub async fn sign_in(email: &str, password: &str) -> anyhow::Result<Self> {
        let client = http_client()?;

        let resp = client
            .post(format!("{API_BASE}/auth-service/v2/whoop/sign-in"))
            .json(&SignInRequest {
                username: email,
                password,
            })
            .send()
            .await
            .context("failed to reach WHOOP auth endpoint")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("authentication failed ({status}): {body}");
        }

        let auth: SignInResponse = resp.json().await.context("invalid auth response")?;
        if let Some(expires) = auth.access_token_expires_in {
            log::info!("authenticated (token expires in {expires}s)");
        }

        Ok(Self {
            client,
            token: auth.access_token,
        })
    }

    pub async fn download_firmware(
        &self,
        device_name: &str,
        current_versions: Vec<ChipFirmware>,
        upgrade_versions: Vec<ChipFirmware>,
    ) -> anyhow::Result<String> {
        let resp = self
            .client
            .post(format!("{API_BASE}/firmware-service/v4/firmware/version"))
            .query(&[("deviceName", device_name)])
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-WHOOP-Device-Platform", "ANDROID")
            .json(&FirmwareRequest {
                current_chip_firmwares: current_versions,
                chip_firmwares_of_upgrade: upgrade_versions,
            })
            .send()
            .await
            .context("failed to reach firmware endpoint")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("firmware download failed ({status}): {body}");
        }

        let fw: FirmwareResponse = resp.json().await.context("invalid firmware response")?;

        if let Some(cfg) = &fw.desired_device_firmware_config {
            log::info!(
                "server config (device: {})",
                cfg.hardware_device.as_deref().unwrap_or("?")
            );
            if let Some(chips) = &cfg.chip_firmwares {
                for c in chips {
                    log::info!("  {}: {}", c.chip_name, c.version);
                }
            }
            if cfg.force_update == Some(true) {
                log::info!("  force_update: true");
            }
        }

        fw.firmware_zip_file
            .or(fw.firmware_file)
            .context("no firmware file found in response")
    }
}

pub fn decode_and_extract(firmware_b64: &str, output_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;

    let zip_bytes = BASE64
        .decode(firmware_b64)
        .context("failed to base64-decode firmware")?;

    if zip_bytes.len() > MAX_FIRMWARE_ZIP_BYTES {
        bail!(
            "decoded firmware is {} bytes (max {}); refusing to process",
            zip_bytes.len(),
            MAX_FIRMWARE_ZIP_BYTES
        );
    }

    log::info!(
        "decoded firmware ZIP: {} bytes ({:.1} KB)",
        zip_bytes.len(),
        zip_bytes.len() as f64 / 1024.0
    );

    let zip_path = output_dir.join("firmware.zip");
    std::fs::write(&zip_path, &zip_bytes)
        .with_context(|| format!("failed to write {}", zip_path.display()))?;
    log::info!("saved ZIP to {}", zip_path.display());

    let cursor = io::Cursor::new(&zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).context("invalid ZIP archive")?;

    let n_entries = archive.len();
    if n_entries > MAX_ZIP_ENTRIES {
        bail!(
            "zip has {} entries (max {}); refusing to extract",
            n_entries,
            MAX_ZIP_ENTRIES
        );
    }

    let root = std::fs::canonicalize(output_dir)
        .with_context(|| format!("failed to canonicalize {}", output_dir.display()))?;

    let mut total_uncompressed: u64 = 0;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;

        if file.encrypted() {
            bail!("zip entry {:?} is encrypted; refusing", file.name());
        }
        if file.is_symlink() {
            bail!("zip entry {:?} is a symlink; refusing", file.name());
        }

        let rel: PathBuf = match file.enclosed_name() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => bail!("zip entry {:?} has unsafe or empty path", file.name()),
        };

        let out_path = root.join(&rel);
        if !out_path.starts_with(&root) {
            bail!(
                "zip entry {:?} resolves outside output directory",
                file.name()
            );
        }

        if file.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }

        if file.size() > MAX_UNCOMPRESSED_ENTRY_BYTES {
            bail!(
                "zip entry {:?} claims {} bytes uncompressed (max {})",
                file.name(),
                file.size(),
                MAX_UNCOMPRESSED_ENTRY_BYTES
            );
        }

        let Some(parent) = out_path.parent() else {
            bail!("zip entry {:?} has no parent path", file.name());
        };
        std::fs::create_dir_all(parent)?;

        let mut out_file = std::fs::File::create(&out_path)?;
        let limit = file.size().min(MAX_UNCOMPRESSED_ENTRY_BYTES);
        let written = io::copy(&mut file.take(limit), &mut out_file)?;
        total_uncompressed = total_uncompressed
            .checked_add(written)
            .context("total uncompressed size overflow")?;
        if total_uncompressed > MAX_TOTAL_UNCOMPRESSED_BYTES {
            bail!(
                "extracted more than {} bytes total; possible zip bomb",
                MAX_TOTAL_UNCOMPRESSED_BYTES
            );
        }

        log::info!("  {} ({} bytes)", rel.display(), written);
    }

    log::info!("firmware files saved to {}/", output_dir.display());
    Ok(())
}
