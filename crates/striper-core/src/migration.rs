//! One-time, non-destructive import of a pre-fork Handy installation.
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: bool,
    pub model_files: usize,
}

/// Import settings and completed model files into a fresh Striper data folder.
/// Never overwrite a Striper settings store, follow symlinks, or modify Handy.
/// Completed staging is renamed into place; interrupted copies stay invisible.
pub fn import_handy(source: &Path, destination: &Path) -> io::Result<ImportReport> {
    let settings_name = "settings_store.json";
    if destination.join(settings_name).exists() || !source.join(settings_name).is_file() {
        return Ok(ImportReport::default());
    }
    if fs::symlink_metadata(source)?.file_type().is_symlink()
        || fs::symlink_metadata(source.join(settings_name))?
            .file_type()
            .is_symlink()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Handy import source must not be a symlink",
        ));
    }
    let mut settings: serde_json::Value =
        serde_json::from_slice(&fs::read(source.join(settings_name))?)?;
    let values = settings
        .get_mut("settings")
        .and_then(|s| s.as_object_mut())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Handy settings are not an object",
            )
        })?;
    // Never inherit Handy's upstream updater opt-in or start a background mic.
    values.insert("update_checks_enabled".into(), false.into());
    values.insert("correction_listener_enabled".into(), false.into());
    fs::create_dir_all(destination)?;
    let staging = destination.join("handy-import-staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir(&staging)?;
    let result = (|| {
        let models = copy_completed_models(&source.join("models"), &staging.join("models"))?;
        fs::write(
            staging.join(settings_name),
            serde_json::to_vec_pretty(&settings)?,
        )?;
        // Commit models before settings: a crash can be retried until the
        // settings file (the commit marker) is present. Existing files win.
        if staging.join("models").is_dir() {
            merge_missing(&staging.join("models"), &destination.join("models"))?;
        }
        // A concurrent app-created settings file wins; hard-link is an atomic
        // create-if-absent on the same filesystem (within destination).
        match fs::hard_link(staging.join(settings_name), destination.join(settings_name)) {
            Ok(()) => Ok(ImportReport {
                imported: true,
                model_files: models,
            }),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(ImportReport::default()),
            Err(e) => Err(e),
        }
    })();
    let _ = fs::remove_dir_all(staging);
    result
}

fn copy_completed_models(source: &Path, destination: &Path) -> io::Result<usize> {
    if !source.exists() {
        return Ok(0);
    }
    if fs::symlink_metadata(source)?.file_type().is_symlink() {
        return Ok(0);
    }
    fs::create_dir_all(destination)?;
    let mut count = 0;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".partial") || ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            count += copy_completed_models(&entry.path(), &destination.join(name))?;
        } else if ty.is_file() {
            fs::copy(entry.path(), destination.join(name))?;
            count += 1;
        }
    }
    Ok(count)
}
fn merge_missing(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() && fs::symlink_metadata(destination)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Import destination must not be a symlink",
        ));
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            merge_missing(&entry.path(), &target)?;
        } else if !target.exists() {
            // Same-volume atomic no-overwrite, without a second model copy.
            match fs::hard_link(entry.path(), target) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_install_preserves_preferences_and_completed_models() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("com.pais.handy");
        let dest = tmp.path().join("com.benstein1.striper");
        fs::create_dir_all(source.join("models/custom")).unwrap();
        fs::write(source.join("settings_store.json"), br#"{"settings":{"selected_model":"whisper-small","update_checks_enabled":true,"selected_microphone":"Dad's mic"}}"#).unwrap();
        fs::write(source.join("models/custom/model.gguf"), b"test model").unwrap();
        fs::write(source.join("models/unfinished.partial"), b"unfinished").unwrap();
        let report = import_handy(&source, &dest).unwrap();
        assert_eq!(
            report,
            ImportReport {
                imported: true,
                model_files: 1
            }
        );
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(dest.join("settings_store.json")).unwrap()).unwrap();
        assert_eq!(settings["settings"]["selected_microphone"], "Dad's mic");
        assert_eq!(settings["settings"]["selected_model"], "whisper-small");
        assert_eq!(settings["settings"]["update_checks_enabled"], false);
        assert_eq!(settings["settings"]["correction_listener_enabled"], false);
        assert_eq!(
            fs::read(dest.join("models/custom/model.gguf")).unwrap(),
            b"test model"
        );
        assert!(!dest.join("models/unfinished.partial").exists());
        assert!(source.join("models/custom/model.gguf").is_file());
        assert!(!import_handy(&source, &dest).unwrap().imported);
    }
    #[test]
    fn existing_striper_store_is_never_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("striper");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("settings_store.json"), "keep me").unwrap();
        assert!(
            !import_handy(&tmp.path().join("missing"), &dest)
                .unwrap()
                .imported
        );
        assert_eq!(
            fs::read_to_string(dest.join("settings_store.json")).unwrap(),
            "keep me"
        );
    }
    #[test]
    fn malformed_source_does_not_leave_a_destination_store() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("settings_store.json"), "bad json").unwrap();
        let dest = tmp.path().join("new");
        assert!(import_handy(tmp.path(), &dest).is_err());
        assert!(!dest.join("settings_store.json").exists());
    }
}
