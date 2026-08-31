//! Tauri command boundary.
//!
//! Commands only return the normalized, secret-free domain model. Credential
//! values stay behind `CredentialResolver` and cannot be serialized.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::account_registry::AccountSnapshotRegistry;
use crate::claude_profiles::{self, ClaudeLoginStarted};
use crate::codex_profiles::{self, CodexLoginStarted};
use crate::models::{AccountDescriptor, ProviderId, QuotaSnapshot};
use crate::providers::credentials::default_descriptors;
use crate::providers::{ProviderContext, ProviderRegistry};
use crate::remediation::{self, AuthorizedAction, RemediationStore};

/// The longest user-selectable refresh interval is five minutes. Any cached
/// value older than that must stop presenting itself as live quota data.
const IN_PROCESS_CACHE_STALE_AFTER_SECONDS: i64 = 5 * 60;

pub struct AppState {
    registry: Arc<ProviderRegistry>,
    context: Arc<ProviderContext>,
    app_data_dir: PathBuf,
    account_registry: RwLock<AccountSnapshotRegistry>,
    remediation: RwLock<RemediationStore>,
    cache: RwLock<Option<CachedSnapshot>>,
}

#[derive(Clone)]
struct CachedSnapshot {
    excluded_account_ids: BTreeSet<String>,
    snapshot: QuotaSnapshot,
}

impl AppState {
    pub fn new(app_data_dir: PathBuf) -> Result<Self, String> {
        let context = ProviderContext::new().map_err(|error| error.to_string())?;
        let account_registry = match AccountSnapshotRegistry::load(&app_data_dir) {
            Ok(registry) => registry,
            Err(_) => {
                // Retained snapshots are an optional, secret-free cache. A
                // damaged or temporarily unreadable cache must never prevent
                // live provider monitoring (or the app itself) from starting.
                eprintln!("EyeUrAI could not read the account snapshot registry");
                AccountSnapshotRegistry::empty(&app_data_dir)
            }
        };
        Ok(Self {
            registry: Arc::new(ProviderRegistry::with_defaults()),
            context: Arc::new(context),
            app_data_dir,
            account_registry: RwLock::new(account_registry),
            remediation: RwLock::new(RemediationStore::default()),
            cache: RwLock::new(None),
        })
    }

    fn cached(&self, excluded_account_ids: &BTreeSet<String>) -> Option<QuotaSnapshot> {
        let cached = self.cache.read().ok()?.clone()?;
        if cached.excluded_account_ids != *excluded_account_ids {
            return None;
        }
        let mut snapshot = cached.snapshot;
        snapshot.refresh_cached(self.context.now(), IN_PROCESS_CACHE_STALE_AFTER_SECONDS);
        Some(snapshot)
    }

    async fn refresh(
        &self,
        only: Option<Vec<ProviderId>>,
        excluded_account_ids: BTreeSet<String>,
    ) -> QuotaSnapshot {
        let mut all_descriptors = default_descriptors();
        match codex_profiles::discover_descriptors(&self.app_data_dir) {
            Ok(mut profiles) => all_descriptors.append(&mut profiles),
            Err(error) => eprintln!("EyeUrAI could not discover Codex profiles: {error}"),
        }
        match claude_profiles::discover_descriptors(&self.app_data_dir) {
            Ok(mut profiles) => all_descriptors.append(&mut profiles),
            Err(error) => eprintln!("EyeUrAI could not discover Claude accounts: {error}"),
        }
        let descriptors = all_descriptors
            .into_iter()
            .filter(|descriptor| !descriptor_is_excluded(descriptor, &excluded_account_ids))
            .collect();
        let mut snapshot = self
            .registry
            .refresh(Arc::clone(&self.context), Arc::new(descriptors), only)
            .await;
        if let Ok(mut registry) = self.account_registry.write() {
            if registry
                .reconcile(&mut snapshot, &excluded_account_ids)
                .is_err()
            {
                // Quota data remains useful when the non-secret cache cannot
                // be written. Avoid including paths or upstream payloads in
                // logs; the next refresh will try persistence again.
                eprintln!("EyeUrAI could not persist the account snapshot registry");
            }
        }
        if let Ok(mut remediation) = self.remediation.write() {
            remediation.install(&mut snapshot);
        }
        if let Ok(mut cache) = self.cache.write() {
            *cache = Some(CachedSnapshot {
                excluded_account_ids,
                snapshot: snapshot.clone(),
            });
        }
        snapshot
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemediationExecution {
    LoginStarted {
        provider: ProviderId,
        #[serde(rename = "profileId")]
        profile_id: String,
        #[serde(rename = "targetAccountId")]
        target_account_id: Option<String>,
    },
    TerminalOpened {
        provider: ProviderId,
        command: String,
        #[serde(rename = "targetAccountId")]
        target_account_id: Option<String>,
    },
    RefreshRequested {
        provider: ProviderId,
    },
    OpenSettings {
        provider: ProviderId,
        #[serde(rename = "openConnection")]
        open_connection: bool,
    },
}

/// Execute one choice from the latest backend-authored diagnosis. Plan ids
/// are replaced on every refresh, so a button from an older account state is
/// rejected instead of performing a now-inappropriate side effect.
#[tauri::command]
pub async fn execute_remediation(
    app: AppHandle,
    plan_id: String,
    choice_id: String,
    state: State<'_, AppState>,
) -> Result<RemediationExecution, String> {
    if plan_id.len() > 80 || choice_id.len() > 40 {
        return Err("This repair action is invalid. Refresh and try again.".to_string());
    }
    let action = state
        .remediation
        .read()
        .ok()
        .and_then(|store| store.resolve(&plan_id, &choice_id))
        .ok_or_else(|| "This repair action is out of date. Refresh and try again.".to_string())?;

    match action {
        AuthorizedAction::ManagedLogin {
            provider: ProviderId::Claude,
            account_id,
        } => {
            let http = state.context.http.clone();
            let user_agent = state.context.user_agent(ProviderId::Claude).to_string();
            let started = claude_profiles::start_login(app, &state.app_data_dir, http, user_agent)
                .await
                .map_err(|error| error.message)?;
            Ok(RemediationExecution::LoginStarted {
                provider: ProviderId::Claude,
                profile_id: started.profile_id,
                target_account_id: account_id,
            })
        }
        AuthorizedAction::ManagedLogin {
            provider: ProviderId::Codex,
            account_id,
        } => {
            let started = codex_profiles::start_login(app, &state.app_data_dir)
                .await
                .map_err(|error| error.message)?;
            Ok(RemediationExecution::LoginStarted {
                provider: ProviderId::Codex,
                profile_id: started.profile_id,
                target_account_id: account_id,
            })
        }
        AuthorizedAction::ManagedLogin { .. } => {
            Err("This provider does not support an EyeUrAI sign-in.".to_string())
        }
        AuthorizedAction::OpenTerminal {
            provider,
            account_id,
        } => {
            let command = remediation::open_terminal(provider, &state.app_data_dir)?;
            Ok(RemediationExecution::TerminalOpened {
                provider,
                command: command.to_string(),
                target_account_id: account_id,
            })
        }
        AuthorizedAction::Retry { provider } => {
            Ok(RemediationExecution::RefreshRequested { provider })
        }
        AuthorizedAction::OpenSettings {
            provider,
            open_connection,
        } => Ok(RemediationExecution::OpenSettings {
            provider,
            open_connection,
        }),
    }
}

/// Start an official Codex browser login in a new, isolated `CODEX_HOME`.
/// The Codex app-server owns token storage and rotation; EyeUrAI receives only
/// a non-secret profile identifier and a completion event.
#[tauri::command]
pub async fn start_codex_account_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CodexLoginStarted, String> {
    codex_profiles::start_login(app, &state.app_data_dir)
        .await
        .map_err(|error| error.message)
}

/// Start Anthropic's official browser sign-in for a new, isolated Claude
/// profile. EyeUrAI owns the resulting grant (requested with a read-only
/// scope) and stores it in that profile only; the terminal's Claude Code
/// login is never touched.
#[tauri::command]
pub async fn start_claude_account_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ClaudeLoginStarted, String> {
    let http = state.context.http.clone();
    let user_agent = state.context.user_agent(ProviderId::Claude).to_string();
    claude_profiles::start_login(app, &state.app_data_dir, http, user_agent)
        .await
        .map_err(|error| error.message)
}

fn normalize_exclusions(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .collect()
}

/// A terminal credential slot has a fixed descriptor ID, but a successful
/// read is exposed under a pseudonymous principal ID. Once that principal is
/// disconnected, exclude both identities so a later logout cannot turn the
/// hidden account back into a `claude-cli`/`codex-cli` failure row.
fn descriptor_is_excluded(
    descriptor: &AccountDescriptor,
    excluded_account_ids: &BTreeSet<String>,
) -> bool {
    if excluded_account_ids.contains(&descriptor.id) {
        return true;
    }

    let principal_prefix = match (descriptor.provider, descriptor.id.as_str()) {
        (ProviderId::Claude, "claude-cli") => Some("claude:"),
        (ProviderId::Codex, "codex-cli") => Some("codex:"),
        _ => None,
    };
    principal_prefix.is_some_and(|prefix| {
        excluded_account_ids
            .iter()
            .any(|account_id| account_id.starts_with(prefix))
    })
}

/// Return the in-memory snapshot, performing the first live read lazily.
#[tauri::command]
pub async fn get_snapshot(
    excluded_account_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<QuotaSnapshot, String> {
    let excluded_account_ids = normalize_exclusions(excluded_account_ids);
    if let Some(snapshot) = state.cached(&excluded_account_ids) {
        return Ok(snapshot);
    }
    Ok(state.refresh(None, excluded_account_ids).await)
}

/// Re-read every configured provider immediately.
#[tauri::command]
pub async fn refresh_quotas(
    app: AppHandle,
    excluded_account_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<QuotaSnapshot, String> {
    let snapshot = state
        .refresh(None, normalize_exclusions(excluded_account_ids))
        .await;
    let _ = app.emit("snapshot-updated", &snapshot);
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions_are_trimmed_deduplicated_and_reject_empty_values() {
        let exclusions = normalize_exclusions(vec![
            " claude-cli ".to_string(),
            "".to_string(),
            "claude-cli".to_string(),
            "codex-cli".to_string(),
        ]);
        assert_eq!(
            exclusions.into_iter().collect::<Vec<_>>(),
            vec!["claude-cli".to_string(), "codex-cli".to_string()]
        );
    }

    #[test]
    fn disconnected_cli_principal_also_excludes_its_mutable_descriptor() {
        let exclusions = normalize_exclusions(vec![
            "claude:principal-fingerprint".to_string(),
            "codex:account-claim".to_string(),
        ]);
        let descriptors = default_descriptors();
        let claude = descriptors
            .iter()
            .find(|descriptor| descriptor.id == "claude-cli")
            .expect("default Claude descriptor");
        let codex = descriptors
            .iter()
            .find(|descriptor| descriptor.id == "codex-cli")
            .expect("default Codex descriptor");

        assert!(descriptor_is_excluded(claude, &exclusions));
        assert!(descriptor_is_excluded(codex, &exclusions));
    }

    #[test]
    fn disconnecting_a_managed_profile_does_not_hide_the_terminal_slot() {
        let exclusions = normalize_exclusions(vec![
            "claude-profile:profile-1".to_string(),
            "codex-profile:profile-1".to_string(),
        ]);
        let descriptors = default_descriptors();

        assert!(descriptors
            .iter()
            .all(|descriptor| !descriptor_is_excluded(descriptor, &exclusions)));
    }

    #[test]
    fn remediation_execution_uses_the_frontend_wire_names() {
        let value = serde_json::to_value(RemediationExecution::LoginStarted {
            provider: ProviderId::Claude,
            profile_id: "profile-1".to_string(),
            target_account_id: Some("claude:principal".to_string()),
        })
        .expect("serialize execution");
        assert_eq!(value["kind"], "login_started");
        assert_eq!(value["profileId"], "profile-1");
        assert_eq!(value["targetAccountId"], "claude:principal");
        assert!(value.get("profile_id").is_none());
    }
}
