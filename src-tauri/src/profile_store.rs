//! Shared storage machinery for EyeUrAI-owned provider profiles.
//!
//! Both Codex and Claude accounts added inside EyeUrAI live in isolated,
//! owner-only profile directories under a versioned root inside the app data
//! directory. This module owns everything about those directories that is not
//! provider-specific: private creation, id validity, path trust validation,
//! and the two-level (in-process + OS file) access lock that serializes
//! logins and refreshes against the same profile.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use tokio::sync::Mutex as AsyncMutex;

use crate::providers::error::ProviderError;

const PROFILE_LOCK_FILE: &str = ".eyeurai-profile.lock";
static PROFILE_NONCE: AtomicU64 = AtomicU64::new(0);
static PROFILE_LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> = OnceLock::new();
const PROFILE_OS_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const PROFILE_OS_LOCK_RETRY: Duration = Duration::from_millis(50);

/// One provider's profile root (e.g. `codex-profiles-v1`) inside app data.
pub struct ProfileStore {
    root: PathBuf,
    root_name: &'static str,
}

/// Held while a profile is being read or written. Dropping releases the OS
/// lock; the in-process lock releases with the guard as well.
pub struct ProfileAccessGuard {
    _in_process: tokio::sync::OwnedMutexGuard<()>,
    lock_file: File,
}

impl Drop for ProfileAccessGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.lock_file);
    }
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    // Unix lock contention normally maps to WouldBlock. Windows returns
    // ERROR_LOCK_VIOLATION, which Rust currently classifies as Uncategorized,
    // so compare fs2's canonical raw error without broadening every
    // Uncategorized I/O failure into a retryable lock conflict.
    error.kind() == std::io::ErrorKind::WouldBlock
        || match (
            error.raw_os_error(),
            fs2::lock_contended_error().raw_os_error(),
        ) {
            (Some(actual), Some(expected)) => actual == expected,
            _ => false,
        }
}

async fn lock_exclusive_with_timeout(lock_file: File, timeout: Duration) -> std::io::Result<File> {
    let deadline = Instant::now() + timeout;
    loop {
        match fs2::FileExt::try_lock_exclusive(&lock_file) {
            Ok(()) => return Ok(lock_file),
            Err(error) if is_lock_contended(&error) => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "profile lock remained busy",
                    ));
                }
                tokio::time::sleep(PROFILE_OS_LOCK_RETRY).await;
            }
            Err(error) => return Err(error),
        }
    }
}

impl ProfileStore {
    pub fn new(app_data_dir: &Path, root_name: &'static str) -> Self {
        ProfileStore {
            root: app_data_dir.join(root_name),
            root_name,
        }
    }

    /// Only exercised from tests today; kept as the store's public shape.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Existing profile directories with valid ids, in stable id order.
    /// A missing root simply means "no profiles yet".
    pub fn profiles(&self) -> Result<Vec<(String, PathBuf)>, ProviderError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => {
                return Err(ProviderError::internal(
                    "could not inspect EyeUrAI profile storage",
                ))
            }
        };

        let mut profiles = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(profile_id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !valid_profile_id(&profile_id) {
                continue;
            }
            profiles.push((profile_id, entry.path()));
        }
        profiles.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(profiles)
    }

    /// Create the root as an owner-only real directory (no symlinks).
    pub fn ensure_root(&self) -> Result<(), ProviderError> {
        create_private_directory(&self.root)
    }

    /// Allocate a fresh owner-only profile directory with a unique id.
    pub fn create_profile(&self) -> Result<(String, PathBuf), ProviderError> {
        for _ in 0..32 {
            let profile_id = unique_profile_id();
            let profile_home = self.root.join(&profile_id);
            match fs::create_dir(&profile_home) {
                Ok(()) => {
                    #[cfg(unix)]
                    fs::set_permissions(&profile_home, fs::Permissions::from_mode(0o700))
                        .map_err(|_| ProviderError::internal("could not secure the new profile"))?;
                    return Ok((profile_id, profile_home));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(ProviderError::internal("could not create a new profile")),
            }
        }
        Err(ProviderError::internal(
            "could not allocate a unique profile",
        ))
    }

    /// Refuse any profile path that is not a real directory directly under
    /// this store's root, with a valid id and no symlink indirection.
    pub fn validate_profile_home(&self, profile_home: &Path) -> Result<(), ProviderError> {
        let profile_id = profile_home
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| valid_profile_id(value))
            .ok_or_else(|| ProviderError::forbidden("the profile identifier is invalid"))?;
        let parent = profile_home
            .parent()
            .ok_or_else(|| ProviderError::forbidden("the profile path has no parent"))?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|_| ProviderError::credentials_missing("the profile root no longer exists"))?;
        if !parent_metadata.file_type().is_dir()
            || parent.file_name().and_then(|value| value.to_str()) != Some(self.root_name)
        {
            return Err(ProviderError::forbidden(
                "the profile is outside EyeUrAI profile storage",
            ));
        }
        let metadata = fs::symlink_metadata(profile_home)
            .map_err(|_| ProviderError::credentials_missing("the profile no longer exists"))?;
        if !metadata.file_type().is_dir() {
            return Err(ProviderError::internal(
                "the profile path is not a directory",
            ));
        }
        let canonical_parent = fs::canonicalize(parent)
            .map_err(|_| ProviderError::internal("could not resolve the profile root"))?;
        let canonical_profile = fs::canonicalize(profile_home)
            .map_err(|_| ProviderError::internal("could not resolve the profile"))?;
        if canonical_profile.parent() != Some(canonical_parent.as_path())
            || canonical_profile
                .file_name()
                .and_then(|value| value.to_str())
                != Some(profile_id)
        {
            return Err(ProviderError::forbidden(
                "the profile path could not be trusted",
            ));
        }
        Ok(())
    }

    /// Serialize access to one profile: an in-process async lock (so one
    /// EyeUrAI cannot interleave its own reads) plus an exclusive OS file
    /// lock (so a second EyeUrAI process cannot either).
    pub async fn lock_profile_access(
        &self,
        profile_home: &Path,
    ) -> Result<ProfileAccessGuard, ProviderError> {
        self.validate_profile_home(profile_home)?;
        let in_process = profile_lock(profile_home)?.lock_owned().await;
        let lock_path = profile_home.join(PROFILE_LOCK_FILE);
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(ProviderError::forbidden(
                    "the profile lock path could not be trusted",
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(ProviderError::internal(
                    "could not inspect the profile lock",
                ))
            }
        }
        let lock_file = {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true);
            #[cfg(unix)]
            options.mode(0o600);
            options
                .open(lock_path)
                .map_err(|_| ProviderError::internal("could not open the profile lock"))?
        };
        let lock_file = lock_exclusive_with_timeout(lock_file, PROFILE_OS_LOCK_TIMEOUT)
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    ProviderError::timeout("the account profile is busy in another EyeUrAI process")
                } else {
                    ProviderError::internal("could not lock the profile")
                }
            })?;
        Ok(ProfileAccessGuard {
            _in_process: in_process,
            lock_file,
        })
    }

    /// Best-effort removal of a profile that never completed its login.
    pub fn remove_incomplete(&self, profile_home: &Path) {
        if profile_home.parent() == Some(self.root.as_path()) {
            let _ = fs::remove_dir_all(profile_home);
        }
    }
}

/// Create `path` as an owner-only real directory, rejecting symlinks.
pub fn create_private_directory(path: &Path) -> Result<(), ProviderError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(ProviderError::forbidden(
                "the profile directory is not a private directory",
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|_| ProviderError::internal("could not create the profile directory"))?;
            let metadata = fs::symlink_metadata(path)
                .map_err(|_| ProviderError::internal("could not inspect the profile directory"))?;
            if !metadata.file_type().is_dir() {
                return Err(ProviderError::forbidden(
                    "the profile directory is not a private directory",
                ));
            }
        }
        Err(_) => {
            return Err(ProviderError::internal(
                "could not inspect the profile directory",
            ))
        }
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| ProviderError::internal("could not secure the profile directory"))?;
    Ok(())
}

fn profile_lock(profile_home: &Path) -> Result<Arc<AsyncMutex<()>>, ProviderError> {
    let canonical = fs::canonicalize(profile_home)
        .map_err(|_| ProviderError::credentials_missing("the profile no longer exists"))?;
    let locks = PROFILE_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .map_err(|_| ProviderError::internal("the profile lock was unavailable"))?;
    Ok(Arc::clone(
        locks
            .entry(canonical)
            .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
    ))
}

pub fn valid_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn unique_profile_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let nonce = PROFILE_NONCE.fetch_add(1, Ordering::Relaxed);
    format!("profile-{nanos:x}-{:x}-{nonce:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) struct TestDirectory(pub PathBuf);

    impl TestDirectory {
        pub(crate) fn new() -> Self {
            TestDirectory(
                std::env::temp_dir().join(format!("eyeurai-profile-store-{}", unique_profile_id())),
            )
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store(temp: &TestDirectory) -> ProfileStore {
        ProfileStore::new(&temp.0, "test-profiles-v1")
    }

    #[cfg(unix)]
    #[test]
    fn profile_directories_are_owner_only() {
        let temp = TestDirectory::new();
        let store = store(&temp);
        store.ensure_root().unwrap();
        let (_, profile) = store.create_profile().unwrap();
        assert_eq!(
            fs::metadata(store.root()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(profile).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = TestDirectory::new();
        fs::create_dir_all(&temp.0).unwrap();
        let redirect = temp.0.join("redirect");
        fs::create_dir(&redirect).unwrap();
        let store = store(&temp);
        symlink(&redirect, store.root()).unwrap();

        assert!(store.ensure_root().is_err());
    }

    #[test]
    fn profiles_outside_the_store_root_are_rejected() {
        let temp = TestDirectory::new();
        let store = store(&temp);
        store.ensure_root().unwrap();
        let (_, profile) = store.create_profile().unwrap();
        store.validate_profile_home(&profile).unwrap();

        let elsewhere = temp.0.join("elsewhere").join("profile-imposter");
        fs::create_dir_all(&elsewhere).unwrap();
        assert!(store.validate_profile_home(&elsewhere).is_err());
        assert!(store.validate_profile_home(&temp.0).is_err());
    }

    #[test]
    fn repeated_profile_access_uses_the_same_in_process_lock() {
        let temp = TestDirectory::new();
        let store = store(&temp);
        store.ensure_root().unwrap();
        let (_, profile) = store.create_profile().unwrap();

        let first = profile_lock(&profile).unwrap();
        let second = profile_lock(&profile).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn profile_access_holds_an_owner_only_os_lock() {
        let temp = TestDirectory::new();
        let store = store(&temp);
        store.ensure_root().unwrap();
        let (_, profile) = store.create_profile().unwrap();

        let guard = store.lock_profile_access(&profile).await.unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(profile.join(PROFILE_LOCK_FILE))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_err());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(profile.join(PROFILE_LOCK_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(guard);
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_ok());
        let _ = fs2::FileExt::unlock(&second);
    }

    #[tokio::test]
    async fn profile_os_lock_wait_has_a_deadline() {
        let temp = TestDirectory::new();
        fs::create_dir_all(&temp.0).unwrap();
        let lock_path = temp.0.join("busy.lock");
        let first = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        fs2::FileExt::lock_exclusive(&first).unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();

        let error = lock_exclusive_with_timeout(second, Duration::from_millis(25))
            .await
            .expect_err("a busy profile lock must time out");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        let _ = fs2::FileExt::unlock(&first);
    }

    #[test]
    fn removal_only_touches_children_of_the_root() {
        let temp = TestDirectory::new();
        let store = store(&temp);
        store.ensure_root().unwrap();
        let (_, profile) = store.create_profile().unwrap();

        let outside = temp.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        store.remove_incomplete(&outside);
        assert!(outside.exists());

        store.remove_incomplete(&profile);
        assert!(!profile.exists());
    }
}
