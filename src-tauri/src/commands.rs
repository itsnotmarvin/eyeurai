//! Tauri command boundary.
//!
//! Commands only return the normalized, secret-free domain model. Credential
//! values stay behind `CredentialResolver` and cannot be serialized.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tauri::{AppHandle, Emitter, State};

use crate::account_registry::AccountSnapshotRegistry;
use crate::codex_profiles::{self, CodexLoginStarted};
use crate::models::{ProviderCapability, ProviderId, QuotaSnapshot};
use crate::providers::credentials::default_descriptors;
use crate::providers::{ProviderContext, ProviderRegistry};

pub struct AppState {
    registry: Arc<ProviderRegistry>,
    context: Arc<ProviderContext>,
    app_data_dir: PathBuf,
    account_registry: RwLock<AccountSnapshotRegistry>,
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
        let account_registry =
            AccountSnapshotRegistry::load(&app_data_dir).map_err(|error| error.to_string())?;
        Ok(Self {
            registry: Arc::new(ProviderRegistry::with_defaults()),
            context: Arc::new(context),
            app_data_dir,
            account_registry: RwLock::new(account_registry),
            cache: RwLock::new(None),
        })
    }

    fn cached(&self, excluded_account_ids: &BTreeSet<String>) -> Option<QuotaSnapshot> {
        let cached = self.cache.read().ok()?.clone()?;
        if cached.excluded_account_ids != *excluded_account_ids {
            return None;
        }
        let mut snapshot = cached.snapshot;
        snapshot.refresh_countdowns(self.context.now());
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
        let descriptors = all_descriptors
            .into_iter()
            .filter(|descriptor| !excluded_account_ids.contains(&descriptor.id))
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
        if let Ok(mut cache) = self.cache.write() {
            *cache = Some(CachedSnapshot {
                excluded_account_ids,
                snapshot: snapshot.clone(),
            });
        }
        snapshot
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

fn normalize_exclusions(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 160)
        .collect()
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

/// Static, non-secret capability metadata for settings and diagnostics.
#[tauri::command]
pub fn provider_capabilities(state: State<'_, AppState>) -> Vec<ProviderCapability> {
    state.registry.capabilities()
}

/// Explicit development aid. Live commands never fall back to demo data, so
/// the UI cannot mistake synthetic values for a real account.
#[tauri::command]
pub fn get_demo_snapshot() -> QuotaSnapshot {
    crate::providers::demo::snapshot(chrono::Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_has_all_provider_capabilities() {
        let state = AppState::new(
            std::env::temp_dir().join(format!("eyeurai-state-test-{}", std::process::id())),
        )
        .unwrap();
        let capabilities = state.registry.capabilities();
        assert_eq!(capabilities.len(), 4);
        assert_eq!(capabilities[0].provider, ProviderId::Claude);
        assert_eq!(capabilities[3].provider, ProviderId::Gemini);
    }

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
}
