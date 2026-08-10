//! Durable, secret-free account quota snapshots.
//!
//! The provider adapters observe whichever account a vendor CLI is currently
//! using. When that principal changes, the old credential is no longer
//! available to this read-only app. This registry retains the last successful
//! normalized snapshot for that principal and replays it as explicitly stale
//! data; it never stores credential descriptors or resolved credentials.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{
    AccountSnapshot, DataSource, Freshness, ProviderErrorInfo, ProviderErrorKind, ProviderId,
    ProviderStatus, QuotaSnapshot,
};

const REGISTRY_SCHEMA_VERSION: u32 = 1;
const REGISTRY_DIRECTORY: &str = "account-snapshots-v1";
const SNAPSHOT_PREFIX: &str = "snapshot-";
const SNAPSHOT_SUFFIX: &str = ".json";

/// Credential-slot IDs identify a mutable CLI location, not a person. They
/// may be rendered for a transient provider/profile failure, but must never be
/// retained as an account because the next terminal login can reuse the same
/// slot for somebody else.
const MUTABLE_CLI_SLOT_IDS: [&str; 2] = ["claude-cli", "codex-cli"];

static FILE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PersistedAccount {
    provider: ProviderId,
    snapshot: AccountSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PersistedRegistry {
    schema_version: u32,
    generation: u64,
    accounts: Vec<PersistedAccount>,
}

/// Last successful snapshots keyed by provider and stable upstream identity.
///
/// The directory contains one committed generation. A save writes and syncs a
/// uniquely named temporary file, atomically renames it into place, then
/// removes older committed generations. If the process stops between the
/// rename and cleanup, loading simply chooses the highest valid generation.
pub struct AccountSnapshotRegistry {
    directory: PathBuf,
    generation: u64,
    accounts: BTreeMap<(ProviderId, String), AccountSnapshot>,
}

impl AccountSnapshotRegistry {
    pub fn load(app_data_dir: impl AsRef<Path>) -> io::Result<Self> {
        let directory = app_data_dir.as_ref().join(REGISTRY_DIRECTORY);
        let Some(persisted) = read_latest_generation(&directory)? else {
            return Ok(AccountSnapshotRegistry {
                directory,
                generation: 0,
                accounts: BTreeMap::new(),
            });
        };

        let accounts = persisted
            .accounts
            .into_iter()
            .filter(|entry| {
                entry.snapshot.account_id.len() <= 160
                    && !entry.snapshot.account_id.trim().is_empty()
                    && !is_mutable_cli_slot(&entry.snapshot.account_id)
                    && entry.snapshot.status.has_data()
            })
            .map(|entry| {
                (
                    (entry.provider, entry.snapshot.account_id.clone()),
                    entry.snapshot,
                )
            })
            .collect();

        Ok(AccountSnapshotRegistry {
            directory,
            generation: persisted.generation,
            accounts,
        })
    }

    /// Merge a live refresh with retained snapshots, then persist every newly
    /// successful account. Excluded IDs are neither returned nor newly stored.
    pub fn reconcile(
        &mut self,
        snapshot: &mut QuotaSnapshot,
        excluded_account_ids: &BTreeSet<String>,
    ) -> io::Result<()> {
        let now = snapshot.generated_at;
        let mut changed = false;

        for provider in &mut snapshot.providers {
            provider
                .accounts
                .retain(|account| !excluded_account_ids.contains(&account.account_id));

            let observed_ids: BTreeSet<String> = provider
                .accounts
                .iter()
                .map(|account| account.account_id.clone())
                .collect();

            for account in &mut provider.accounts {
                if account.status == ProviderStatus::Ok
                    && account.freshness.source == DataSource::Live
                    && !is_mutable_cli_slot(&account.account_id)
                {
                    let mut retained = account.clone();
                    // "Active" describes the current CLI selection, not the
                    // historical quota payload, so it is deliberately not
                    // asserted in durable storage.
                    retained.active = false;
                    retained.error = None;
                    let key = (provider.provider, retained.account_id.clone());
                    changed |= self.accounts.get(&key) != Some(&retained);
                    self.accounts.insert(key, retained);
                } else if let Some(stored) = self
                    .accounts
                    .get(&(provider.provider, account.account_id.clone()))
                {
                    // A transient provider/API failure for an account whose
                    // identity is still known must not throw away its last
                    // successful windows. Keep the current failure detail and
                    // active-CLI flag while replaying the cached quota values.
                    let current_error = account.error.clone();
                    let current_active = account.active;
                    let mut retained = stale_copy(stored, provider.provider, now);
                    retained.active = current_active;
                    if current_error.is_some() {
                        retained.error = current_error;
                    }
                    *account = retained;
                }
            }

            let historical: Vec<AccountSnapshot> = self
                .accounts
                .iter()
                .filter(|((stored_provider, account_id), _)| {
                    *stored_provider == provider.provider
                        && !observed_ids.contains(account_id)
                        && !excluded_account_ids.contains(account_id)
                })
                .map(|(_, account)| stale_copy(account, provider.provider, now))
                .collect();

            provider.accounts.extend(historical);
            provider.status = provider.derive_status();
            provider.sort_accounts();
        }

        if changed {
            self.persist()?;
        }
        Ok(())
    }

    fn persist(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;

        let generation = self.generation.saturating_add(1);
        let accounts = self
            .accounts
            .iter()
            .map(|((provider, _), snapshot)| PersistedAccount {
                provider: *provider,
                snapshot: snapshot.clone(),
            })
            .collect();
        let persisted = PersistedRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation,
            accounts,
        };
        let bytes = serde_json::to_vec_pretty(&persisted).map_err(io::Error::other)?;

        let nonce = unique_nonce();
        let temporary = self
            .directory
            .join(format!(".{SNAPSHOT_PREFIX}{generation:020}-{nonce}.tmp"));
        let committed = self.directory.join(format!(
            "{SNAPSHOT_PREFIX}{generation:020}-{nonce}{SNAPSHOT_SUFFIX}"
        ));

        let write_result = (|| -> io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &committed)?;
            sync_directory(&self.directory)?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
            return write_result;
        }

        self.generation = generation;

        // Cleanup occurs only after the new generation is durably committed.
        // Failure here is harmless: the loader understands multiple versions.
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path != committed && is_committed_snapshot(&path) {
                    let _ = fs::remove_file(path);
                }
            }
        }

        Ok(())
    }
}

fn is_mutable_cli_slot(account_id: &str) -> bool {
    MUTABLE_CLI_SLOT_IDS.contains(&account_id)
}

fn stale_copy(
    stored: &AccountSnapshot,
    provider: ProviderId,
    now: DateTime<Utc>,
) -> AccountSnapshot {
    let mut account = stored.clone();
    account.active = false;
    account.status = ProviderStatus::Error;
    account.error = Some(ProviderErrorInfo::new(
        ProviderErrorKind::CredentialsMissing,
        match provider {
            ProviderId::Claude => {
                "This account is not the current Claude Code login. Its last successful quota snapshot is retained but cannot be refreshed independently."
            }
            ProviderId::Codex => {
                "This account is not the current Codex CLI login. Its last successful quota snapshot is retained but isolated per-account refresh is not available yet."
            }
            _ => {
                "This account is no longer available through its configured credential. Its last successful quota snapshot is retained."
            }
        },
    ));

    let fetched_at = stored.freshness.fetched_at;
    account.freshness = Freshness {
        source: DataSource::Cache,
        fetched_at,
        age_seconds: fetched_at.map(|at| (now - at).num_seconds().max(0)),
        // Unavailability makes the row stale immediately, independent of the
        // ordinary time-based cache budget.
        stale: true,
    };
    for window in &mut account.windows {
        window.refresh_countdown(now);
    }
    account
}

fn read_latest_generation(directory: &Path) -> io::Result<Option<PersistedRegistry>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut latest: Option<PersistedRegistry> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_committed_snapshot(&path) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(candidate) = serde_json::from_slice::<PersistedRegistry>(&bytes) else {
            continue;
        };
        if candidate.schema_version != REGISTRY_SCHEMA_VERSION {
            continue;
        }
        if latest
            .as_ref()
            .map(|current| candidate.generation > current.generation)
            .unwrap_or(true)
        {
            latest = Some(candidate);
        }
    }
    Ok(latest)
}

fn is_committed_snapshot(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(SNAPSHOT_SUFFIX))
        .unwrap_or(false)
}

fn unique_nonce() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{time}-{count}", std::process::id())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    fs::File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        CapabilityLevel, CredentialKind, ProviderCapability, ProviderSnapshot, QuotaWindow,
        WindowKind,
    };
    use chrono::TimeZone;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("eyeurai-{name}-{}", unique_nonce()));
            TestDirectory(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().expect("valid time")
    }

    fn capability(provider: ProviderId) -> ProviderCapability {
        ProviderCapability {
            provider,
            display_name: provider.display_name().to_string(),
            level: CapabilityLevel::Full,
            data_source: "test".to_string(),
            official_api: false,
            read_only: true,
            supports_multiple_accounts: true,
            supports_percent: true,
            supports_reset_times: true,
            supports_currency: false,
            credential_kinds: vec![CredentialKind::None],
            option_keys: vec![],
            notes: vec![],
            doc_url: None,
        }
    }

    fn live_snapshot(
        provider: ProviderId,
        id: &str,
        label: &str,
        used_percent: f64,
        now: DateTime<Utc>,
    ) -> QuotaSnapshot {
        let mut account = AccountSnapshot::new(id, label);
        account.active = true;
        account.freshness = Freshness::live(now);
        account.windows = vec![QuotaWindow::new("five_hour", "5-hour", WindowKind::Rolling)
            .with_used_percent(used_percent)];
        let provider = ProviderSnapshot::new(capability(provider)).with_accounts(vec![account]);
        QuotaSnapshot::new(now, DataSource::Live, vec![provider])
    }

    #[test]
    fn account_switch_retains_a_without_relabeling_and_keeps_b_live() {
        let directory = TestDirectory::new("switch");
        let mut registry = AccountSnapshotRegistry::load(&directory.0).expect("load empty");
        let excluded = BTreeSet::new();

        let mut first = live_snapshot(
            ProviderId::Codex,
            "codex:acct-a",
            "a@example.com",
            31.0,
            ts(1_000),
        );
        registry
            .reconcile(&mut first, &excluded)
            .expect("persist A");

        let mut switched = live_snapshot(
            ProviderId::Codex,
            "codex:acct-b",
            "b@example.com",
            8.0,
            ts(1_120),
        );
        registry
            .reconcile(&mut switched, &excluded)
            .expect("persist B");

        let accounts = &switched.providers[0].accounts;
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].account_id, "codex:acct-b");
        assert_eq!(accounts[0].label, "b@example.com");
        assert!(accounts[0].active);
        assert_eq!(accounts[0].status, ProviderStatus::Ok);
        assert_eq!(accounts[0].freshness.source, DataSource::Live);

        let retained_a = accounts
            .iter()
            .find(|account| account.account_id == "codex:acct-a")
            .expect("A retained");
        assert_eq!(retained_a.label, "a@example.com");
        assert_eq!(retained_a.windows[0].used_percent, Some(31.0));
        assert!(!retained_a.active);
        assert_eq!(retained_a.status, ProviderStatus::Error);
        assert_eq!(retained_a.freshness.source, DataSource::Cache);
        assert!(retained_a.freshness.stale);
        assert!(retained_a.error.is_some());
        assert_eq!(switched.providers[0].status, ProviderStatus::Partial);
    }

    #[test]
    fn retained_accounts_survive_registry_restart() {
        let directory = TestDirectory::new("restart");
        let excluded = BTreeSet::new();
        {
            let mut registry = AccountSnapshotRegistry::load(&directory.0).expect("load empty");
            let mut first = live_snapshot(
                ProviderId::Claude,
                "claude:user-a",
                "first@example.com",
                64.0,
                ts(2_000),
            );
            registry
                .reconcile(&mut first, &excluded)
                .expect("persist first account");
        }

        let mut restarted = AccountSnapshotRegistry::load(&directory.0).expect("reload registry");
        let mut current = live_snapshot(
            ProviderId::Claude,
            "claude:user-b",
            "second@example.com",
            12.0,
            ts(2_300),
        );
        restarted
            .reconcile(&mut current, &excluded)
            .expect("merge after restart");

        let retained = current.providers[0]
            .accounts
            .iter()
            .find(|account| account.account_id == "claude:user-a")
            .expect("persisted account survives restart");
        assert_eq!(retained.label, "first@example.com");
        assert_eq!(retained.freshness.age_seconds, Some(300));
        assert!(retained.freshness.stale);
    }

    #[test]
    fn persisted_registry_contains_snapshots_but_no_credential_material() {
        let directory = TestDirectory::new("no-secrets");
        let mut registry = AccountSnapshotRegistry::load(&directory.0).expect("load empty");
        let mut snapshot = live_snapshot(
            ProviderId::Codex,
            "codex:subject-123",
            "safe@example.com",
            25.0,
            ts(3_000),
        );
        registry
            .reconcile(&mut snapshot, &BTreeSet::new())
            .expect("persist");

        let committed = fs::read_dir(directory.0.join(REGISTRY_DIRECTORY))
            .expect("registry directory")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| is_committed_snapshot(path))
            .expect("committed registry");
        let json = fs::read_to_string(committed).expect("read registry");

        assert!(json.contains("codex:subject-123"));
        for forbidden in [
            "FAKE-access-token",
            "FAKE-refresh-token",
            "access_token",
            "refresh_token",
            "id_token",
            "credential",
        ] {
            assert!(!json.contains(forbidden), "registry leaked {forbidden}");
        }

        #[cfg(unix)]
        {
            let directory_mode = fs::metadata(directory.0.join(REGISTRY_DIRECTORY))
                .expect("registry metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(directory_mode, 0o700);

            let file_mode = fs::metadata(
                fs::read_dir(directory.0.join(REGISTRY_DIRECTORY))
                    .expect("registry directory")
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|path| is_committed_snapshot(path))
                    .expect("committed registry"),
            )
            .expect("snapshot metadata")
            .permissions()
            .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
        }
    }

    #[test]
    fn mutable_cli_slot_ids_are_never_retained_as_accounts() {
        let directory = TestDirectory::new("mutable-slot");
        let mut registry = AccountSnapshotRegistry::load(&directory.0).expect("load empty");
        let mut ambiguous = live_snapshot(
            ProviderId::Claude,
            "claude-cli",
            "Claude Code login",
            18.0,
            ts(4_000),
        );
        registry
            .reconcile(&mut ambiguous, &BTreeSet::new())
            .expect("reconcile ambiguous slot");

        let mut switched = live_snapshot(
            ProviderId::Claude,
            "claude:user-b",
            "b@example.com",
            22.0,
            ts(4_100),
        );
        registry
            .reconcile(&mut switched, &BTreeSet::new())
            .expect("reconcile stable account");

        assert_eq!(switched.providers[0].accounts.len(), 1);
        assert_eq!(
            switched.providers[0].accounts[0].account_id,
            "claude:user-b"
        );
    }

    #[test]
    fn transient_failure_keeps_last_successful_windows_for_same_account() {
        let directory = TestDirectory::new("transient-failure");
        let mut registry = AccountSnapshotRegistry::load(&directory.0).expect("load empty");
        let excluded = BTreeSet::new();
        let account_id = "codex:stable-account";

        let mut live = live_snapshot(
            ProviderId::Codex,
            account_id,
            "dev@example.com",
            47.0,
            ts(5_000),
        );
        registry
            .reconcile(&mut live, &excluded)
            .expect("persist live snapshot");

        let mut failed_account = AccountSnapshot::failed(
            account_id,
            "dev@example.com",
            ProviderErrorInfo::new(ProviderErrorKind::Timeout, "provider timed out"),
        );
        failed_account.active = true;
        let provider = ProviderSnapshot::new(capability(ProviderId::Codex))
            .with_accounts(vec![failed_account]);
        let mut failed = QuotaSnapshot::new(ts(5_120), DataSource::Live, vec![provider]);

        registry
            .reconcile(&mut failed, &excluded)
            .expect("merge cached snapshot after failure");

        let account = &failed.providers[0].accounts[0];
        assert!(account.active);
        assert_eq!(account.windows[0].used_percent, Some(47.0));
        assert_eq!(account.freshness.source, DataSource::Cache);
        assert!(account.freshness.stale);
        assert_eq!(
            account.error.as_ref().map(|error| error.kind),
            Some(ProviderErrorKind::Timeout)
        );
        assert_eq!(
            account.error.as_ref().map(|error| error.message.as_str()),
            Some("provider timed out")
        );
    }
}
