use crate::application::desired_state::DesiredState;
use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_FILENAME: &str = "desired-state.json";

pub fn default_path() -> PathBuf {
    PathBuf::from(DEFAULT_FILENAME)
}

pub fn write(path: &Path, desired_state: &DesiredState) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "could not create desired-state directory {}",
            parent.display()
        )
    })?;

    let mut json =
        serde_json::to_vec_pretty(desired_state).context("could not serialize desired state")?;
    json.push(b'\n');

    let temporary_path = temporary_path(path)?;
    write_temporary(&temporary_path, &json)?;

    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error).with_context(|| {
            format!(
                "could not atomically replace desired-state file {}",
                path.display()
            )
        });
    }
    Ok(())
}

fn write_temporary(path: &Path, json: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "could not create temporary desired-state file {}",
                path.display()
            )
        })?;
    let result = (|| {
        file.write_all(json).with_context(|| {
            format!(
                "could not write temporary desired-state file {}",
                path.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "could not flush temporary desired-state file {}",
                path.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_nanos();
    let filename = path
        .file_name()
        .context("desired-state path must include a file name")?
        .to_string_lossy();
    Ok(path.with_file_name(format!(".{filename}.{}.{}.tmp", std::process::id(), nonce)))
}
