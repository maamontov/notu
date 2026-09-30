use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile};

use crate::config::DAILY_TAG;

#[derive(Clone, Debug)]
pub struct Note {
    pub title: String,
    pub path: PathBuf,
    pub created: DateTime<Local>,
}

pub struct Workspace {
    pub root: PathBuf,
    created: RefCell<BTreeMap<String, i64>>,
    pub warning: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct DeletedNote {
    name: String,
    created: i64,
    deleted: i64,
}

impl Workspace {
    pub fn open(root: Option<PathBuf>) -> Result<Self> {
        let root = match root {
            Some(root) => root,
            None => dirs::document_dir()
                .or_else(|| dirs::home_dir().map(|home| home.join("Documents")))
                .context("Cannot find Documents directory; use --workspace PATH")?
                .join("notu"),
        };
        fs::create_dir_all(&root)
            .with_context(|| format!("Cannot create workspace {}", root.display()))?;
        let root = fs::canonicalize(root)?;
        let dates_path = root.join(".notu-created.json");
        let mut warning = None;
        let created = match fs::read(&dates_path) {
            Ok(bytes) => {
                let parsed = serde_json::from_slice::<BTreeMap<String, i64>>(&bytes);
                match parsed {
                    Ok(dates)
                        if dates
                            .values()
                            .all(|&value| DateTime::from_timestamp_millis(value).is_some()) =>
                    {
                        dates
                    }
                    _ => {
                        warning = Some(crate::recovery::backup(&dates_path, &bytes)?);
                        BTreeMap::new()
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error).context("Cannot read note creation dates"),
        };
        let store = Self {
            root,
            created: RefCell::new(created),
            warning,
        };
        if store.warning.is_some() {
            store.save_dates()?;
        }
        Ok(store)
    }

    fn creation(&self, path: &Path) -> Result<DateTime<Local>> {
        let name = path
            .file_name()
            .context("Invalid note filename")?
            .to_str()
            .context("Filename must be UTF-8")?;
        if let Some(&timestamp) = self.created.borrow().get(name) {
            return DateTime::from_timestamp_millis(timestamp)
                .map(|date| date.with_timezone(&Local))
                .context("Invalid note creation timestamp");
        }
        let metadata = fs::metadata(path)?;
        let created: DateTime<Local> = metadata.created().or_else(|_| metadata.modified())?.into();
        self.created
            .borrow_mut()
            .insert(name.to_owned(), created.timestamp_millis());
        Ok(created)
    }

    fn save_dates(&self) -> Result<()> {
        let mut file = NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(&mut file, &*self.created.borrow())?;
        file.as_file().sync_all()?;
        file.persist(self.root.join(".notu-created.json"))
            .map_err(|error| error.error)
            .context("Cannot save note creation dates")?;
        Ok(())
    }

    // Index names only: opening the app does not read every note's contents.
    pub fn list(&self) -> Result<Vec<Note>> {
        let mut notes = Vec::new();
        let indexed = self.created.borrow().len();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                let title = path
                    .file_stem()
                    .context("Invalid note filename")?
                    .to_str()
                    .context("Note filenames must be UTF-8")?
                    .to_owned();
                let created = self.creation(&path)?;
                notes.push(Note {
                    title,
                    path,
                    created,
                });
            }
        }
        if self.created.borrow().len() != indexed {
            self.save_dates()?;
        }
        notes.sort_by(|a, b| {
            b.created
                .cmp(&a.created)
                .then_with(|| b.title.cmp(&a.title))
        });
        Ok(notes)
    }

    pub fn today(&self) -> Result<Note> {
        let date = Local::now().date_naive();
        let title = format!("{DAILY_TAG}: {}", date.format("%d-%m-%Y"));
        let mut note = self.note(&title)?;
        // Reuse today's note from older versions, preserving its content and creation date.
        if matches!(fs::symlink_metadata(&note.path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            let mut legacy = self.note(&date.format("%Y-%m-%d").to_string())?;
            match fs::symlink_metadata(&legacy.path) {
                Ok(_) => {
                    self.read(&legacy)?;
                    legacy.created = self.creation(&legacy.path)?;
                    return self.rename(&legacy, &title);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("Cannot check today's existing note"),
            }
        }
        match self.create(&title) {
            Ok(note) => Ok(note),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                // Refuse symlinks, directories and unreadable existing files.
                self.read(&note)?;
                note.created = self.creation(&note.path)?;
                Ok(note)
            }
            Err(error) => Err(error),
        }
    }

    fn note(&self, title: &str) -> Result<Note> {
        let title = title.trim();
        if title.is_empty()
            || title == "."
            || title == ".."
            || title.ends_with('.')
            || title
                .chars()
                .any(|c| c.is_control() || "<>\"/\\|?*".contains(c) || (cfg!(windows) && c == ':'))
            || title.len() > 180
        {
            bail!("Use a title of 1–180 bytes without forbidden filename or control characters");
        }
        // Windows device names are reserved even when an extension is present.
        let base = title
            .split('.')
            .next()
            .unwrap_or(title)
            .to_ascii_uppercase();
        if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            bail!("This title is a reserved filename");
        }
        Ok(Note {
            title: title.to_owned(),
            path: self.root.join(format!("{title}.md")),
            created: Local::now(),
        })
    }

    pub fn create(&self, title: &str) -> Result<Note> {
        let note = self.note(title)?;
        let file = NamedTempFile::new_in(&self.root)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&note.path)
            .map_err(|error| error.error)
            .with_context(|| format!("Cannot create {}", note.path.display()))?;
        self.created.borrow_mut().insert(
            note.path.file_name().unwrap().to_str().unwrap().to_owned(),
            note.created.timestamp_millis(),
        );
        self.save_dates()?;
        Ok(note)
    }

    pub fn rename(&self, note: &Note, title: &str) -> Result<Note> {
        let mut renamed = self.note(title)?;
        renamed.created = note.created;
        if renamed.path == note.path {
            return Ok(renamed);
        }
        let content = self.read(note)?;
        let mut file = NamedTempFile::new_in(&self.root)?;
        file.as_file()
            .set_permissions(fs::metadata(&note.path)?.permissions())?;
        file.write_all(content.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&renamed.path)
            .map_err(|error| error.error)
            .context("Cannot rename: target name may already exist")?;
        self.created.borrow_mut().insert(
            renamed
                .path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            note.created.timestamp_millis(),
        );
        self.save_dates()?;
        if self.read(note)? != content {
            bail!("File changed during rename; both files are kept");
        }
        fs::remove_file(&note.path).context("Cannot remove old filename; both files are kept")?;
        Ok(renamed)
    }

    pub fn read(&self, note: &Note) -> Result<String> {
        if !fs::symlink_metadata(&note.path)?.file_type().is_file() {
            bail!("Note must be a regular file: {}", note.path.display());
        }
        fs::read_to_string(&note.path)
            .with_context(|| format!("Cannot read {} as UTF-8", note.path.display()))
    }

    pub fn delete(&self, note: &Note, expected: &str) -> Result<()> {
        if self.read(note)? != expected {
            bail!("File changed outside notu. Deletion cancelled; reload and confirm again");
        }
        let name = note
            .path
            .file_name()
            .context("Invalid note filename")?
            .to_str()
            .context("Filename must be UTF-8")?
            .to_owned();
        let trash = self.root.join(".notu-trash");
        if !trash.exists() {
            fs::create_dir(&trash)?;
        }
        ensure!(
            fs::symlink_metadata(&trash)?.file_type().is_dir(),
            "Trash must be a regular directory"
        );
        // Keep a synced local copy before unlinking. A failed deletion drops the temporary entry.
        let archive = Builder::new().prefix("deleted-").tempdir_in(&trash)?;
        let body_path = archive.path().join("body.md");
        let mut body = fs::File::create(&body_path)?;
        body.set_permissions(fs::metadata(&note.path)?.permissions())?;
        body.write_all(expected.as_bytes())?;
        body.sync_all()?;
        let record = DeletedNote {
            name: name.clone(),
            created: note.created.timestamp_millis(),
            deleted: chrono::Utc::now().timestamp_micros(),
        };
        let mut metadata = fs::File::create(archive.path().join("note.json"))?;
        serde_json::to_writer(&mut metadata, &record)?;
        metadata.sync_all()?;
        let timestamp = self.created.borrow_mut().remove(&name);
        // Finish the fallible index write before unlinking the confirmed note.
        if let Err(error) = self.save_dates() {
            if let Some(timestamp) = timestamp {
                self.created.borrow_mut().insert(name, timestamp);
            }
            return Err(error);
        }
        let result = (|| -> Result<()> {
            if self.read(note)? != expected {
                bail!("File changed during deletion; the note is kept");
            }
            fs::remove_file(&note.path).context("Cannot delete note")
        })();
        if let Err(error) = result {
            if let Some(timestamp) = timestamp {
                self.created.borrow_mut().insert(name, timestamp);
            }
            self.save_dates()
                .context("Cannot restore creation dates after failed deletion")?;
            return Err(error);
        }
        let _ = archive.keep();
        Ok(())
    }

    pub fn restore(&self) -> Result<Note> {
        let trash = self.root.join(".notu-trash");
        ensure!(
            fs::symlink_metadata(&trash).is_ok_and(|meta| meta.file_type().is_dir()),
            "No deleted notes to restore"
        );
        let mut entries = Vec::new();
        for entry in fs::read_dir(&trash)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let folder = entry.path();
            let metadata = folder.join("note.json");
            let body = folder.join("body.md");
            if !fs::symlink_metadata(&metadata).is_ok_and(|m| m.file_type().is_file())
                || !fs::symlink_metadata(&body).is_ok_and(|m| m.file_type().is_file())
            {
                continue;
            }
            if let Ok(record) = serde_json::from_slice::<DeletedNote>(&fs::read(metadata)?) {
                entries.push((record, folder));
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.0.deleted));
        let (record, folder) = entries.first().context("No deleted notes to restore")?;
        let relative = Path::new(&record.name);
        ensure!(
            relative.file_name() == Some(relative.as_os_str())
                && relative
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
            "Invalid archived filename"
        );
        let title = relative
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Invalid archived title")?
            .to_owned();
        let created = DateTime::from_timestamp_millis(record.created)
            .context("Invalid archived creation date")?
            .with_timezone(&Local);
        let note = Note {
            title,
            path: self.root.join(relative),
            created,
        };
        let body = folder.join("body.md");
        let mut file = NamedTempFile::new_in(&self.root)?;
        file.as_file()
            .set_permissions(fs::metadata(&body)?.permissions())?;
        file.write_all(&fs::read(body)?)?;
        file.as_file().sync_all()?;
        ensure!(
            matches!(fs::symlink_metadata(&note.path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "Cannot restore: original filename already exists or cannot be checked"
        );
        let previous = self
            .created
            .borrow_mut()
            .insert(record.name.clone(), record.created);
        let rollback = || {
            if let Some(previous) = previous {
                self.created
                    .borrow_mut()
                    .insert(record.name.clone(), previous);
            } else {
                self.created.borrow_mut().remove(&record.name);
            }
        };
        if let Err(error) = self.save_dates() {
            rollback();
            return Err(error);
        }
        if let Err(error) = file.persist_noclobber(&note.path) {
            rollback();
            self.save_dates()
                .context("Cannot roll back creation dates after failed restore")?;
            return Err(error.error).context("Cannot restore: original filename may already exist");
        }
        fs::remove_dir_all(folder).context("Note restored, but cannot remove its archived copy")?;
        Ok(note)
    }

    pub fn save(&self, note: &Note, expected: &str, content: &str) -> Result<()> {
        if self.read(note)? != expected {
            bail!(
                "File changed outside notu. Your edits are kept in memory; save a copy with Ctrl+N"
            );
        }
        let permissions = fs::metadata(&note.path)?.permissions();
        let mut file = NamedTempFile::new_in(&self.root)?;
        file.as_file().set_permissions(permissions)?;
        file.write_all(content.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&note.path)
            .map_err(|error| error.error)
            .with_context(|| format!("Cannot save {}", note.path.display()))?;
        #[cfg(unix)]
        fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_last_in_first_out_and_never_overwrites_existing_notes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let first = store.create("First")?;
        let second = store.create("Second")?;
        store.save(&first, "", "First body")?;
        store.save(&second, "", "Second body")?;
        store.delete(&first, "First body")?;
        store.delete(&second, "Second body")?;
        let replacement = store.create("Second")?;
        store.save(&replacement, "", "Replacement")?;
        assert!(store.restore().is_err());
        assert_eq!(store.read(&replacement)?, "Replacement");
        // Simulate resolving the filename collision outside the app.
        fs::rename(&replacement.path, dir.path().join("Replacement.md"))?;
        let restored = store.restore()?;
        assert_eq!(restored.path, second.path);
        assert_eq!(
            restored.created.timestamp_millis(),
            second.created.timestamp_millis()
        );
        assert_eq!(store.read(&restored)?, "Second body");
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let restored = store.restore()?;
        assert_eq!(restored.path, first.path);
        assert_eq!(store.read(&restored)?, "First body");
        assert!(store.restore().is_err());
        Ok(())
    }

    #[test]
    fn failed_restore_metadata_write_leaves_archive_usable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("Keep")?;
        store.delete(&note, "")?;
        fs::remove_file(dir.path().join(".notu-created.json"))?;
        fs::create_dir(dir.path().join(".notu-created.json"))?;
        assert!(store.restore().is_err());
        assert!(!note.path.exists());
        fs::remove_dir(dir.path().join(".notu-created.json"))?;
        assert_eq!(store.restore()?.path, note.path);
        Ok(())
    }

    #[test]
    fn damaged_creation_index_recovers_and_preserves_the_original_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Keep.md"), "Body")?;
        for bytes in [
            b"broken".as_slice(),
            br#"{"Keep.md":9223372036854775807}"#.as_slice(),
        ] {
            fs::write(dir.path().join(".notu-created.json"), bytes)?;
            let store = Workspace::open(Some(dir.path().to_owned()))?;
            assert!(store.warning.is_some());
            assert_eq!(store.list()?.len(), 1);
            assert_eq!(fs::read_to_string(dir.path().join("Keep.md"))?, "Body");
            let backups: Vec<_> = fs::read_dir(dir.path())?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(".notu-created.json.corrupt-")
                })
                .collect();
            assert!(
                backups
                    .iter()
                    .any(|entry| fs::read(entry.path()).unwrap() == bytes)
            );
        }
        assert!(
            Workspace::open(Some(dir.path().to_owned()))?
                .warning
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn unicode_notes_roundtrip_and_atomic_save() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().join("notes")))?;
        let note = store.create("Идеи 🦀")?;
        let original = store.read(&note)?;
        assert_eq!(original, "");
        store.save(&note, &original, "# Идеи 🦀\n\n**Привет**\n")?;
        assert_eq!(store.read(&note)?, "# Идеи 🦀\n\n**Привет**\n");
        assert_eq!(store.list()?.len(), 1);
        assert!(store.create("Идеи 🦀").is_err());
        Ok(())
    }

    #[test]
    fn daily_note_is_idempotent() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let first = store.today()?;
        assert_eq!(
            first.title,
            format!("daily: {}", Local::now().format("%d-%m-%Y"))
        );
        let original = store.read(&first)?;
        assert!(original.is_empty());
        store.save(&first, &original, "Diary\n")?;
        assert_eq!(first.path, store.today()?.path);
        assert_eq!(store.read(&first)?, "Diary\n");
        assert_eq!(store.list()?.len(), 1);
        Ok(())
    }

    #[test]
    fn legacy_daily_note_is_reused_without_overwriting_either_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let old_title = Local::now().format("%Y-%m-%d").to_string();
        let old = store.create(&old_title)?;
        store.save(&old, "", "Diary from the old version\n")?;
        let daily = store.today()?;
        assert!(!old.path.exists());
        assert_eq!(store.read(&daily)?, "Diary from the old version\n");
        assert_eq!(
            daily.created.timestamp_millis(),
            old.created.timestamp_millis()
        );
        let other = store.create(&old_title)?;
        store.save(&other, "", "Another file")?;
        assert_eq!(store.today()?.path, daily.path);
        assert_eq!(store.read(&daily)?, "Diary from the old version\n");
        assert_eq!(store.read(&other)?, "Another file");
        Ok(())
    }

    #[test]
    fn rename_preserves_content_and_creation_date_without_overwriting() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let original = store.create("Old")?;
        let content = "# Independent heading\n\n**Содержимое**\n";
        store.save(&original, "", content)?;
        let existing = store.create("Existing")?;
        store.save(&existing, "", "keep me")?;
        assert!(store.rename(&original, "Existing").is_err());
        assert!(store.rename(&original, "../invalid").is_err());
        assert_eq!(store.read(&original)?, content);
        assert_eq!(store.read(&existing)?, "keep me");
        let renamed = store.rename(&original, "Новое имя")?;
        assert!(!original.path.exists());
        assert_eq!(store.read(&renamed)?, content);
        assert_eq!(renamed.created, original.created);
        store.save(&renamed, content, "updated")?;
        let reopened = Workspace::open(Some(dir.path().to_owned()))?;
        let notes = reopened.list()?;
        assert_eq!(notes.len(), 2);
        let persisted = notes.iter().find(|note| note.title == "Новое имя").unwrap();
        assert_eq!(
            persisted.created.timestamp_millis(),
            original.created.timestamp_millis()
        );
        assert_eq!(reopened.read(persisted)?, "updated");
        Ok(())
    }

    #[test]
    fn refuses_invalid_paths_and_external_changes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        for title in ["", "../escape", "a/b", "a\\b", "a\nb", "..", "CON", "LPT1"] {
            assert!(store.create(title).is_err(), "accepted {title:?}");
        }
        let note = store.create("test")?;
        let original = store.read(&note)?;
        fs::write(&note.path, "External edit")?;
        assert!(store.save(&note, &original, "My edit").is_err());
        assert_eq!(store.read(&note)?, "External edit");
        Ok(())
    }

    #[test]
    fn delete_checks_content_and_removes_creation_metadata() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("Delete me")?;
        store.save(&note, "", "Confirmed content")?;
        assert!(store.delete(&note, "old content").is_err());
        assert_eq!(store.read(&note)?, "Confirmed content");
        store.delete(&note, "Confirmed content")?;
        assert!(!note.path.exists());
        assert!(store.created.borrow().is_empty());
        let reopened = Workspace::open(Some(dir.path().to_owned()))?;
        assert!(reopened.created.borrow().is_empty());
        assert!(reopened.list()?.is_empty());
        Ok(())
    }

    #[test]
    fn failed_metadata_write_keeps_the_note_and_its_creation_date() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("Keep me")?;
        let dates = store.created.borrow().clone();
        let index = dir.path().join(".notu-created.json");
        fs::remove_file(&index)?;
        fs::create_dir(&index)?;
        assert!(store.delete(&note, "").is_err());
        assert_eq!(store.read(&note)?, "");
        assert_eq!(*store.created.borrow(), dates);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinks_and_preserves_permissions() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("real")?;
        fs::set_permissions(&note.path, fs::Permissions::from_mode(0o640))?;
        let original = store.read(&note)?;
        store.save(&note, &original, "updated")?;
        assert_eq!(
            fs::metadata(&note.path)?.permissions().mode() & 0o777,
            0o640
        );
        symlink(&note.path, store.root.join("link.md"))?;
        assert_eq!(store.list()?.len(), 1);
        assert!(
            store
                .read(&Note {
                    title: "link".into(),
                    path: store.root.join("link.md"),
                    created: Local::now()
                })
                .is_err()
        );
        assert!(
            store
                .delete(
                    &Note {
                        title: "link".into(),
                        path: store.root.join("link.md"),
                        created: Local::now()
                    },
                    "updated"
                )
                .is_err()
        );
        assert_eq!(store.read(&note)?, "updated");
        Ok(())
    }
}
