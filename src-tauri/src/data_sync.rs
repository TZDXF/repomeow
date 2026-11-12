//! Durable CLI invalidation, scoped to the selected data root. The OS releases
//! the writer lock on crash; the intent revision then exposes partial commits.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager};

pub(crate) type Revisions = BTreeMap<String, String>;

pub(crate) fn lock_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

/// Replace a complete UTF-8 document; concurrent writers never share a temp file.
pub(crate) fn atomic_write(path: &Path, body: &[u8]) -> std::io::Result<()> {
    static NEXT_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::write(&temporary, body)?;
    let result = fs::rename(&temporary, path);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn revisions(root: &Path) -> std::io::Result<Revisions> {
    match fs::read(root.join("cli-revisions.json")) {
        Ok(raw) => Ok(serde_json::from_slice(&raw)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Revisions::new()),
        Err(error) => Err(error),
    }
}

pub(crate) fn begin_write(root: &Path, domains: &[&str]) -> std::io::Result<File> {
    let guard = lock_file(&root.join("cli-write.lock"))?;
    let mut current = revisions(root)?;
    let revision = (current
        .values()
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1)
    .to_string();
    for domain in domains {
        current.insert((*domain).into(), revision.clone());
    }
    // Persist intent before any business write. A failed command may have made
    // partial progress; re-reading authoritative data is safe even if it did not.
    atomic_write(
        &root.join("cli-revisions.json"),
        &serde_json::to_vec(&current)?,
    )?;
    Ok(guard)
}

pub(crate) fn snapshot(root: &Path) -> std::io::Result<Option<Revisions>> {
    let path = root.join("cli-write.lock");
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(Revisions::new()))
        }
        Err(error) => return Err(error),
    };
    match file.try_lock_shared() {
        Ok(()) => revisions(root).map(Some),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// All desktop paths, including plugin-store, are derived from this command.
#[tauri::command]
pub(crate) fn get_app_data_dir(app: AppHandle) -> crate::error::AppResult<PathBuf> {
    crate::app_data_dir(&app)
}

#[tauri::command]
pub(crate) fn cli_data_revisions(app: AppHandle) -> crate::error::AppResult<Option<Revisions>> {
    let current = snapshot(&crate::app_data_dir(&app)?)?;
    if let Some(current) = &current {
        let state = app.state::<std::sync::Mutex<Revisions>>();
        let mut previous = state.lock().unwrap();
        if *previous != *current {
            if previous.get("file") != current.get("file") {
                let db = app.state::<crate::db::Db>();
                let conn = db.0.lock().unwrap();
                let mut statement = conn.prepare("SELECT path FROM projects")?;
                for path in statement.query_map([], |row| row.get::<_, String>(0))? {
                    let path = path?;
                    crate::commands::walk::invalidate(Path::new(&path));
                    crate::commands::git::invalidate_status(&path);
                }
            }
            *previous = current.clone();
            app.state::<crate::commands::report::ScheduleNotify>()
                .0
                .notify_one();
            app.state::<crate::commands::git::GitMonitorNotify>()
                .0
                .notify_one();
            let _ = app.emit("data://cli-changed", current);
        }
    }
    Ok(current)
}

pub(crate) fn monitor(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            interval.tick().await;
            let app = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || cli_data_revisions(app)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_wait_for_writer_and_failed_or_crashed_writes_still_invalidate() {
        let root = std::env::temp_dir().join(format!(
            "repomeow-sync-{}",
            crate::time_util::now_ts_nanos()
        ));
        let other = root.join("isolated");
        let guard = begin_write(&root, &["project", "tag"]).unwrap();
        assert!(snapshot(&root).unwrap().is_none());
        assert_eq!(snapshot(&other).unwrap(), Some(Revisions::new()));
        drop(guard); // Also what happens when a command fails, or a process exits.
        let first = snapshot(&root).unwrap().unwrap();
        assert!(first.contains_key("tag"));
        let guard = begin_write(&root, &["project"]).unwrap();
        drop(guard);
        let second = snapshot(&root).unwrap().unwrap();
        assert_ne!(first["project"], second["project"]);
        assert_eq!(first["tag"], second["tag"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_writers_preserve_all_domains() {
        let root = std::env::temp_dir().join(format!(
            "repomeow-sync-{}",
            crate::time_util::now_ts_nanos()
        ));
        std::thread::scope(|scope| {
            for domain in ["project", "tag", "wiki", "prompt"] {
                let root = &root;
                scope.spawn(move || {
                    let _guard = begin_write(root, &[domain]).unwrap();
                });
            }
        });
        assert_eq!(snapshot(&root).unwrap().unwrap().len(), 4);
        fs::remove_dir_all(root).unwrap();
    }
}
