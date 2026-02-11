//! Cache locking for shared caches
//!
//! Per PLAN.md: `shared` caches MUST use a lock to prevent concurrent writers
//! corrupting state. Lock MUST have a timeout and emit diagnostics if
//! contention occurs.
//!
//! This module implements advisory file locking with:
//! - Configurable timeout
//! - Contention logging
//! - Automatic cleanup on drop

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Lock result type
pub type LockResult<T> = Result<T, LockError>;

/// Errors from lock operations
#[derive(Debug, Error)]
pub enum LockError {
    #[error("lock timeout after {0:?}")]
    Timeout(Duration),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("lock file not found: {0}")]
    NotFound(PathBuf),
}

/// Advisory file lock for cache directories.
///
/// The lock is automatically released when this struct is dropped.
pub struct CacheLock {
    /// Path to the lock file
    lock_path: PathBuf,
    /// The opened lock file (held for the lock duration)
    #[allow(dead_code)]
    lock_file: File,
}

impl CacheLock {
    /// Lock file name
    const LOCK_FILENAME: &'static str = ".rch_cache.lock";

    /// Acquire a lock on the given cache directory.
    ///
    /// Creates the directory and lock file if they don't exist.
    /// Waits up to `timeout` for the lock to become available.
    ///
    /// # Arguments
    /// * `cache_dir` - The cache directory to lock
    /// * `timeout` - Maximum time to wait for the lock
    ///
    /// # Returns
    /// * `Ok(CacheLock)` - The lock was acquired
    /// * `Err(LockError::Timeout)` - Timeout waiting for lock
    pub fn acquire(cache_dir: &Path, timeout: Duration) -> LockResult<Self> {
        // Ensure cache directory exists
        fs::create_dir_all(cache_dir)?;

        let lock_path = cache_dir.join(Self::LOCK_FILENAME);
        let start = Instant::now();
        let poll_interval = Duration::from_millis(50);
        let mut warned = false;

        loop {
            // Try to acquire exclusive lock
            match Self::try_acquire_exclusive(&lock_path) {
                Ok(file) => {
                    if warned {
                        eprintln!(
                            "[cache] Lock acquired after {:.1}s contention: {}",
                            start.elapsed().as_secs_f64(),
                            lock_path.display()
                        );
                    }
                    return Ok(Self {
                        lock_path,
                        lock_file: file,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Lock is held by another process
                    if !warned && start.elapsed() > Duration::from_millis(500) {
                        eprintln!(
                            "[cache] WARNING: Lock contention on {}, waiting...",
                            lock_path.display()
                        );
                        warned = true;
                    }
                }
                Err(e) => return Err(LockError::Io(e)),
            }

            // Check timeout
            if start.elapsed() >= timeout {
                return Err(LockError::Timeout(timeout));
            }

            std::thread::sleep(poll_interval);
        }
    }

    /// Try to acquire an exclusive lock on the file.
    #[cfg(unix)]
    fn try_acquire_exclusive(lock_path: &Path) -> io::Result<File> {
        use std::os::unix::fs::OpenOptionsExt;

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(lock_path)?;

        // Try non-blocking exclusive lock
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();

        let result = unsafe {
            libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB)
        };

        if result == 0 {
            Ok(file)
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "lock held"))
            } else {
                Err(err)
            }
        }
    }

    /// Try to acquire an exclusive lock on the file (Windows fallback).
    #[cfg(not(unix))]
    fn try_acquire_exclusive(lock_path: &Path) -> io::Result<File> {
        // On non-Unix, use simpler file-based locking
        // Try to create the file exclusively
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(file) => Ok(file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "lock held"))
            }
            Err(e) => Err(e),
        }
    }

    /// Get the lock file path.
    pub fn path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        // The lock is automatically released when the file is closed.
        // We could optionally delete the lock file here, but leaving it
        // is fine for advisory locking.
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.lock_file.as_raw_fd();
            unsafe {
                libc::flock(fd, libc::LOCK_UN);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lock_acquire_basic() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");

        let lock = CacheLock::acquire(&cache_dir, Duration::from_secs(1)).unwrap();

        // Lock file should exist
        assert!(lock.path().exists());
        assert!(lock.path().file_name().unwrap() == ".rch_cache.lock");
    }

    #[test]
    fn test_lock_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("nested").join("cache");

        assert!(!cache_dir.exists());

        let _lock = CacheLock::acquire(&cache_dir, Duration::from_secs(1)).unwrap();

        assert!(cache_dir.exists());
    }

    #[test]
    fn test_lock_released_on_drop() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");

        // Acquire and release lock
        {
            let _lock = CacheLock::acquire(&cache_dir, Duration::from_secs(1)).unwrap();
        }

        // Should be able to acquire again immediately
        let _lock2 = CacheLock::acquire(&cache_dir, Duration::from_secs(1)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn test_lock_contention() {
        use std::sync::mpsc;
        use std::thread;

        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let cache_dir2 = cache_dir.clone();

        // Acquire lock in main thread
        let lock1 = CacheLock::acquire(&cache_dir, Duration::from_secs(1)).unwrap();

        // Try to acquire in another thread with short timeout
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = CacheLock::acquire(&cache_dir2, Duration::from_millis(100));
            tx.send(result.is_err()).unwrap();
        });

        // Second acquisition should timeout
        let timed_out = rx.recv().unwrap();
        assert!(timed_out, "Second lock acquisition should timeout");

        handle.join().unwrap();
        drop(lock1);
    }

    #[test]
    fn test_lock_timeout_error() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");

        // Create a situation where we can test timeout
        // (This test mainly verifies the timeout path compiles correctly)
        let timeout = Duration::from_millis(100);

        // First lock should succeed
        let _lock = CacheLock::acquire(&cache_dir, timeout).unwrap();
    }
}
