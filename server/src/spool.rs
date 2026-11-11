//! Originals waiting to be sealed and sent to S3.
//!
//! The phone stops waiting 60 seconds after it finishes sending a file. Sealing and the
//! Backblaze PUT can take longer than that, so the upload response is sent while the bytes
//! are still in this directory. Playback reads them here until the PUT finishes.

use std::path::{Path, PathBuf};

use crate::db::DbError;

pub(crate) fn directory(data_dir: &Path, asset_id: &str) -> PathBuf {
    data_dir.join("spool").join(asset_id)
}

pub(crate) fn original_file(data_dir: &Path, asset_id: &str) -> PathBuf {
    directory(data_dir, asset_id).join("original")
}

pub(crate) fn sidecar_file(data_dir: &Path, asset_id: &str) -> PathBuf {
    directory(data_dir, asset_id).join("sidecar.xmp")
}

pub(crate) fn id_from_original_key(key: &str) -> Option<&str> {
    let mut parts = key.split('/');
    let owner = parts.next()?;
    let id = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_none() && !owner.is_empty() && name == "original" {
        Some(id)
    } else {
        None
    }
}

pub(crate) fn remove_directory(data_dir: &Path, asset_id: &str) {
    let _ = std::fs::remove_dir_all(directory(data_dir, asset_id));
}

/// Move `from` onto `to`. A rename stays on the same filesystem. A copy is the fallback.
pub(crate) async fn move_file(from: &Path, to: &Path) -> Result<(), DbError> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| DbError::Store(error.to_string()))?;
    }
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            tokio::fs::copy(from, to)
                .await
                .map_err(|error| DbError::Store(error.to_string()))?;
            let _ = tokio::fs::remove_file(from).await;
            Ok(())
        }
        Err(error) => Err(DbError::Store(error.to_string())),
    }
}
