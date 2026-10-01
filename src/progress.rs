use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::PathBuf,
};

use crate::herdr::socket_path;

// Scope progress and the reset lock to the server, never to a managed plugin checkout.
fn paths() -> Result<(PathBuf, PathBuf), String> {
    let socket = socket_path()?;
    Ok((
        format!("{socket}.pi-reloader-status").into(),
        format!("{socket}.pi-reloader-lock").into(),
    ))
}

pub(crate) struct Progress {
    path: PathBuf,
    _lock: File,
}

impl Progress {
    pub(crate) fn start() -> Result<Self, String> {
        let (path, lock_path) = paths()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|error| error.to_string())?;
        lock.try_lock().map_err(|error| {
            format!("Cannot start reset (another reset may be running): {error}")
        })?;
        let progress = Self { path, _lock: lock };
        progress.update(0, 0, "scanning")?;
        Ok(progress)
    }

    pub(crate) fn update(&self, completed: usize, total: usize, stage: &str) -> Result<(), String> {
        let filled = completed.min(total) * 8 / total.max(1);
        let stage: String = stage.chars().filter(|c| !c.is_control()).take(40).collect();
        let text = format!(
            "DON'T TYPE | Pi [{}{}] {completed}/{total} {stage}",
            "#".repeat(filled),
            "-".repeat(8 - filled)
        );
        let temporary = self.path.with_extension("pi-reloader-status.tmp");
        fs::write(&temporary, text)
            .and_then(|_| fs::rename(&temporary, &self.path))
            .map_err(|error| format!("Cannot publish reset progress: {error}"))
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("pi-reloader-status.tmp"));
        // The OS releases the lock even on a crash; keep its inode for future readers.
    }
}

pub(crate) fn status_line() -> String {
    let read = || -> Result<String, Box<dyn std::error::Error>> {
        let (path, lock_path) = paths()?;
        let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
        match lock.try_lock() {
            Err(TryLockError::WouldBlock) => Ok(fs::read_to_string(path)?),
            Ok(()) => Ok(String::new()), // No reset, including a process that crashed.
            Err(TryLockError::Error(error)) => Err(Box::new(error)),
        }
    };
    read().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn atomic_progress_and_lock_cleanup() -> io::Result<()> {
        let path = std::env::temp_dir().join(format!(
            "pi-progress-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let lock_path = path.with_extension("lock");
        let lock = File::create(&lock_path)?;
        lock.try_lock().unwrap();
        let competing = File::open(&lock_path)?;
        assert!(matches!(
            competing.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        let progress = Progress {
            path: path.clone(),
            _lock: lock,
        };
        progress.update(1, 2, "w2:p1 starting\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path)?,
            "DON'T TYPE | Pi [####----] 1/2 w2:p1 starting"
        );
        drop(progress);
        assert!(!path.exists());
        competing.try_lock().unwrap();
        fs::remove_file(lock_path)?;
        Ok(())
    }
}
