//! Explicit one-click installation of pinned, checksum-verified CPU assets.
//! Nothing is downloaded at startup or merely by visiting settings.
use super::listener;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

pub struct InstallLock(pub AtomicBool);
const MODEL_URL: &str = "https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip";
const MODEL_SHA: &str = "30f26242c4eb449f948e42cb302dd7a686cb29a3423a8367f99ff41780942498";

async fn download(url: &str, sha: &str) -> Result<Vec<u8>, String> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if data.len() + chunk.len() > 100 * 1024 * 1024 {
            return Err("Listener download exceeds its size limit".into());
        }
        data.extend_from_slice(&chunk);
    }
    if format!("{:x}", Sha256::digest(&data)) != sha {
        return Err("Listener download checksum mismatch; no files were installed".into());
    }
    Ok(data)
}

fn extract(data: Vec<u8>, destination: &Path) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let relative = entry
            .enclosed_name()
            .ok_or("Invalid listener archive path")?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("Symlinks are not allowed in listener archives".into());
        }
        total = total.saturating_add(entry.size());
        if total > 512 * 1024 * 1024 {
            return Err("Listener archive exceeds its extraction limit".into());
        }
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut file = std::fs::File::create(output).map_err(|e| e.to_string())?;
            // Bound the actual bytes too, independently of the ZIP header.
            let copied = std::io::copy(&mut entry.by_ref().take(512 * 1024 * 1024 + 1), &mut file)
                .map_err(|e| e.to_string())?;
            if copied > 512 * 1024 * 1024 {
                return Err("Oversized listener archive entry".into());
            }
            file.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn install_correction_listener(app: AppHandle) -> Result<(), String> {
    let lock = app.state::<InstallLock>();
    if lock.0.swap(true, Ordering::AcqRel) {
        return Err("Listener setup is already running".into());
    }
    struct Unlock<'a>(&'a AtomicBool);
    impl Drop for Unlock<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _unlock = Unlock(&lock.0);
    if crate::settings::get_settings(&app).correction_listener_enabled {
        return Err("Turn off the listener before installing its files".into());
    }
    let (library, model) = listener::assets(&app)?;
    if library.is_file() && model.is_dir() {
        return Ok(());
    }
    let (url, sha) = if cfg!(target_os = "windows") {
        (
            "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-win64-0.3.45.zip",
            "f1dcc9cca460630f81ea8f71794f69c80bed6556d2a4e6237b5785e1d2dff34b",
        )
    } else {
        ("https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip", "bbdc8ed85c43979f6443142889770ea95cbfbc56cffb5c5dcd73afa875c5fbb2")
    };
    let native = download(url, sha).await?;
    let acoustic = download(MODEL_URL, MODEL_SHA).await?;
    let base = crate::portable::app_data_dir(&app).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let staging = base.join("listener-installing");
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let result = (|| {
            extract(native, &staging)?;
            extract(acoustic, &staging)?;
            let library_relative = library
                .strip_prefix(base.join("listener"))
                .map_err(|e| e.to_string())?;
            let model_relative = model
                .strip_prefix(base.join("listener"))
                .map_err(|e| e.to_string())?;
            // Validate the actual library/model before exposing the install.
            drop(striper_listener::Detector::open(
                &staging.join(library_relative),
                &staging.join(model_relative),
            )?);
            let destination = base.join("listener");
            if destination.exists() {
                std::fs::remove_dir_all(&destination).map_err(|e| e.to_string())?;
            }
            std::fs::rename(&staging, destination).map_err(|e| e.to_string())
        })();
        if staging.exists() {
            let _ = std::fs::remove_dir_all(staging);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_parent_traversal() {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("../outside", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"no").unwrap();
        let data = archive.finish().unwrap().into_inner();
        let dir = tempfile::tempdir().unwrap();
        assert!(extract(data, dir.path()).is_err());
        assert!(!dir.path().parent().unwrap().join("outside").exists());
    }
}
