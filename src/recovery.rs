use std::{io::Write, path::Path};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::NamedTempFile;

/// Keep malformed metadata intact before rebuilding it. Never overwrite a backup.
pub fn backup(path: &Path, bytes: &[u8]) -> Result<String> {
    let name = path
        .file_name()
        .context("Invalid metadata filename")?
        .to_string_lossy();
    let destination = path.with_file_name(format!(
        "{name}.corrupt-{}",
        Utc::now()
            .timestamp_nanos_opt()
            .context("Invalid backup timestamp")?
    ));
    let mut file = NamedTempFile::new_in(path.parent().context("Invalid metadata directory")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(&destination)
        .map_err(|error| error.error)
        .context("Cannot back up damaged metadata")?;
    Ok(format!(
        "Warning: recovered {}; backup: {}",
        name,
        destination.display()
    ))
}
