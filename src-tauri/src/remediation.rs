//! Central remediation policy and trusted execution helpers.
//!
//! Provider adapters report facts (`ProviderErrorKind`, active identity, and
//! freshness). This module alone decides whether a user action is appropriate.
//! The webview receives an opaque plan id and display copy. A fixed command may
//! be shown for copying, but executable text is always selected again in Rust;
//! the webview cannot alter it before asking the backend to act.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::models::{
    ProviderErrorInfo, ProviderErrorKind, ProviderId, QuotaSnapshot, RemediationChoice,
    RemediationChoiceKind, RemediationImpact, RemediationPlan,
};

const MANAGED_LOGIN: &str = "managed-login";
const OPEN_TERMINAL: &str = "open-terminal";
const RETRY: &str = "retry";
const OPEN_SETTINGS: &str = "open-settings";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizedAction {
    ManagedLogin {
        provider: ProviderId,
        account_id: Option<String>,
    },
    OpenTerminal {
        provider: ProviderId,
        account_id: Option<String>,
    },
    Retry {
        provider: ProviderId,
    },
    OpenSettings {
        provider: ProviderId,
        open_connection: bool,
    },
}

#[derive(Default)]
pub struct RemediationStore {
    plans: BTreeMap<String, BTreeMap<String, AuthorizedAction>>,
}

impl RemediationStore {
    /// Replace every previously authorized plan. A refresh therefore revokes
    /// buttons created from older diagnoses automatically.
    pub fn install(&mut self, snapshot: &mut QuotaSnapshot) {
        self.plans.clear();
        for provider in &mut snapshot.providers {
            provider.remediation_plan =
                provider_plan(provider.provider, provider.error.as_ref(), &mut self.plans);

            for account in &mut provider.accounts {
                account.remediation_plan = account_plan(
                    provider.provider,
                    &account.account_id,
                    account.active,
                    account.error.as_ref(),
                    &mut self.plans,
                );
            }
        }
    }

    pub fn resolve(&self, plan_id: &str, choice_id: &str) -> Option<AuthorizedAction> {
        self.plans.get(plan_id)?.get(choice_id).cloned()
    }
}

fn provider_plan(
    provider: ProviderId,
    error: Option<&ProviderErrorInfo>,
    store: &mut BTreeMap<String, BTreeMap<String, AuthorizedAction>>,
) -> Option<RemediationPlan> {
    let error = error?;
    if retry_plan_allowed(error) {
        return Some(retry_plan(provider, None, store));
    }
    match (provider, error.kind) {
        (ProviderId::Claude | ProviderId::Codex, ProviderErrorKind::CredentialsMissing) => {
            Some(reconnect_plan(provider, None, false, store))
        }
        (ProviderId::OpenRouter, ProviderErrorKind::CredentialsMissing) => {
            Some(settings_plan(provider, false, store))
        }
        _ => None,
    }
}

fn account_plan(
    provider: ProviderId,
    account_id: &str,
    active: bool,
    error: Option<&ProviderErrorInfo>,
    store: &mut BTreeMap<String, BTreeMap<String, AuthorizedAction>>,
) -> Option<RemediationPlan> {
    let error = error?;
    if retry_plan_allowed(error) {
        return Some(retry_plan(provider, Some(account_id), store));
    }

    let retained_cli = !active
        && match provider {
            ProviderId::Claude => account_id.starts_with("claude:"),
            ProviderId::Codex => account_id.starts_with("codex:"),
            _ => false,
        };
    let mutable_cli_slot = matches!(account_id, "claude-cli" | "codex-cli");
    let managed_profile = matches!(provider, ProviderId::Claude | ProviderId::Codex)
        && (account_id.starts_with("claude-profile:") || account_id.starts_with("codex-profile:"));
    let needs_login = matches!(
        error.kind,
        ProviderErrorKind::CredentialsMissing
            | ProviderErrorKind::Unauthorized
            | ProviderErrorKind::Forbidden
    );
    if needs_login && managed_profile {
        // Re-running the global CLI login cannot repair an isolated profile,
        // and silently creating another profile would leave a duplicate error
        // row. Route to the provider-specific connection manager instead.
        return Some(settings_plan(provider, true, store));
    }
    if needs_login && matches!(provider, ProviderId::Claude | ProviderId::Codex) {
        if !retained_cli && !mutable_cli_slot {
            return None;
        }
        return Some(reconnect_plan(
            provider,
            Some(account_id),
            retained_cli,
            store,
        ));
    }
    if needs_login && provider == ProviderId::OpenRouter {
        return Some(settings_plan(provider, false, store));
    }
    None
}

fn retry_plan_allowed(error: &ProviderErrorInfo) -> bool {
    error.retryable && error.kind != ProviderErrorKind::RateLimited
}

fn reconnect_plan(
    provider: ProviderId,
    account_id: Option<&str>,
    retained_cli: bool,
    store: &mut BTreeMap<String, BTreeMap<String, AuthorizedAction>>,
) -> RemediationPlan {
    let plan_id = random_id();
    let provider_name = match provider {
        ProviderId::Claude => "Claude",
        ProviderId::Codex => "Codex",
        _ => unreachable!("only CLI providers have reconnect plans"),
    };
    let command = cli_command(provider);
    let managed_label = if retained_cli {
        "Reconnect inside EyeUrAI"
    } else {
        "Sign in again"
    };
    let terminal_label = match provider {
        ProviderId::Claude => "Switch Claude Code account…",
        ProviderId::Codex => "Switch Codex CLI account…",
        _ => unreachable!(),
    };
    let mut choices = vec![RemediationChoice {
        choice_id: MANAGED_LOGIN.to_string(),
        kind: RemediationChoiceKind::ManagedLogin,
        label: managed_label.to_string(),
        detail: Some(
            "Uses the provider's official browser sign-in and does not change your terminal account."
                .to_string(),
        ),
        command_preview: None,
        impact: RemediationImpact::AppOnly,
    }];
    choices.push(RemediationChoice {
        choice_id: OPEN_TERMINAL.to_string(),
        kind: RemediationChoiceKind::OpenTerminal,
        label: terminal_label.to_string(),
        detail: Some(format!(
            "Opens Terminal and waits for your confirmation before running `{command}`. This changes the account used by {provider_name} on this computer."
        )),
        command_preview: Some(command.to_string()),
        impact: RemediationImpact::GlobalCliIdentity,
    });

    let account_id = account_id.map(str::to_string);
    store.insert(
        plan_id.clone(),
        BTreeMap::from([
            (
                MANAGED_LOGIN.to_string(),
                AuthorizedAction::ManagedLogin {
                    provider,
                    account_id: account_id.clone(),
                },
            ),
            (
                OPEN_TERMINAL.to_string(),
                AuthorizedAction::OpenTerminal {
                    provider,
                    account_id,
                },
            ),
        ]),
    );

    RemediationPlan {
        plan_id,
        title: if retained_cli {
            format!("Reconnect this {provider_name} account?")
        } else {
            format!("Sign in to {provider_name} again?")
        },
        detail: if retained_cli {
            "EyeUrAI remembers this account's last quota snapshot, but it cannot refresh with the current login."
                .to_string()
        } else {
            "The saved credential is missing, expired, or no longer authorized.".to_string()
        },
        choices,
    }
}

fn retry_plan(
    provider: ProviderId,
    _account_id: Option<&str>,
    store: &mut BTreeMap<String, BTreeMap<String, AuthorizedAction>>,
) -> RemediationPlan {
    let plan_id = random_id();
    store.insert(
        plan_id.clone(),
        BTreeMap::from([(RETRY.to_string(), AuthorizedAction::Retry { provider })]),
    );
    RemediationPlan {
        plan_id,
        title: format!("Try {} again?", provider.display_name()),
        detail: "EyeUrAI will request a fresh read from the provider.".to_string(),
        choices: vec![RemediationChoice {
            choice_id: RETRY.to_string(),
            kind: RemediationChoiceKind::Retry,
            label: "Try again".to_string(),
            detail: None,
            command_preview: None,
            impact: RemediationImpact::ReadOnly,
        }],
    }
}

fn settings_plan(
    provider: ProviderId,
    manages_existing: bool,
    store: &mut BTreeMap<String, BTreeMap<String, AuthorizedAction>>,
) -> RemediationPlan {
    let plan_id = random_id();
    store.insert(
        plan_id.clone(),
        BTreeMap::from([(
            OPEN_SETTINGS.to_string(),
            AuthorizedAction::OpenSettings {
                provider,
                open_connection: !manages_existing,
            },
        )]),
    );
    RemediationPlan {
        plan_id,
        title: if manages_existing {
            format!("Manage the {} connection?", provider.display_name())
        } else {
            format!("Connect {}?", provider.display_name())
        },
        detail: "EyeUrAI will take you to the provider's connection instructions.".to_string(),
        choices: vec![RemediationChoice {
            choice_id: OPEN_SETTINGS.to_string(),
            kind: RemediationChoiceKind::OpenSettings,
            label: if manages_existing {
                "Manage connection".to_string()
            } else {
                "Open connection settings".to_string()
            },
            detail: None,
            command_preview: None,
            impact: RemediationImpact::AppOnly,
        }],
    }
}

pub fn cli_command(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Claude => "claude /login",
        ProviderId::Codex => "codex login",
        _ => "",
    }
}

/// Open a terminal with a wrapper that explains the global side effect and
/// waits for Enter. The script and command are selected here from an enum; no
/// webview-supplied shell text reaches the process boundary.
pub fn open_terminal(provider: ProviderId, app_data_dir: &Path) -> Result<&'static str, String> {
    let command = cli_command(provider);
    if command.is_empty() {
        return Err("This provider does not have a terminal sign-in action.".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let script = write_unix_wrapper(provider, app_data_dir, command, "command")
            .map_err(|_| "EyeUrAI could not prepare the Terminal sign-in helper.".to_string())?;
        Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&script)
            .spawn()
            .map_err(|_| {
                "EyeUrAI could not open Terminal. Copy the command instead.".to_string()
            })?;
        return Ok(command);
    }

    #[cfg(target_os = "linux")]
    {
        let script = write_unix_wrapper(provider, app_data_dir, command, "sh")
            .map_err(|_| "EyeUrAI could not prepare the terminal sign-in helper.".to_string())?;
        Command::new("x-terminal-emulator")
            .arg("-e")
            .arg(&script)
            .spawn()
            .map_err(|_| {
                "EyeUrAI could not open a terminal. Copy the command instead.".to_string()
            })?;
        return Ok(command);
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", "cmd", "/K", command])
            .spawn()
            .map_err(|_| {
                "EyeUrAI could not open Command Prompt. Copy the command instead.".to_string()
            })?;
        return Ok(command);
    }

    #[allow(unreachable_code)]
    Err("EyeUrAI cannot open a terminal on this platform. Copy the command instead.".to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_unix_wrapper(
    provider: ProviderId,
    app_data_dir: &Path,
    command: &str,
    extension: &str,
) -> io::Result<PathBuf> {
    let directory = app_data_dir.join("remediation");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let path = directory.join(format!("login-{}.{}", random_id(), extension));
    let provider_name = match provider {
        ProviderId::Claude => "Claude Code",
        ProviderId::Codex => "Codex CLI",
        _ => return Err(io::Error::other("unsupported provider")),
    };
    let content = format!(
        "#!/bin/zsh\nset -u\ntrap 'rm -f -- \"$0\"' EXIT\nprintf '\\nEyeUrAI account repair\\n\\n'\nprintf 'This will change the active {provider_name} account on this computer.\\n'\nprintf 'Command: {command}\\n\\n'\nprintf 'Press Enter to continue, or close this window to cancel. ' \nread -r\nprintf '\\n'\n{command}\nprintf '\\nYou can return to EyeUrAI now.\\n'\n"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o700);
    let mut file = options.open(&path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // A local plan id is not a credential. The fallback still produces a
        // one-process unique value if the operating-system RNG is unavailable.
        return format!(
            "plan-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
    }
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountSnapshot, Freshness, ProviderSnapshot, ProviderStatus};

    fn snapshot(provider: ProviderId, account: AccountSnapshot) -> QuotaSnapshot {
        let capability = match provider {
            ProviderId::Claude => crate::providers::claude::ClaudeProvider::capability_info(),
            ProviderId::Codex => crate::providers::codex::CodexProvider::capability_info(),
            ProviderId::OpenRouter => {
                crate::providers::openrouter::OpenRouterProvider::capability_info()
            }
            ProviderId::Gemini => crate::providers::gemini::GeminiProvider::capability_info(),
        };
        QuotaSnapshot::new(
            chrono::Utc::now(),
            crate::models::DataSource::Cache,
            vec![ProviderSnapshot::new(capability).with_accounts(vec![account])],
        )
    }

    #[test]
    fn retained_claude_account_gets_safe_and_global_choices() {
        let mut account = AccountSnapshot::failed(
            "claude:principal",
            "person@example.com",
            ProviderErrorInfo::new(ProviderErrorKind::CredentialsMissing, "not active"),
        );
        account.freshness = Freshness::none();
        account.active = false;
        let mut snapshot = snapshot(ProviderId::Claude, account);
        let mut store = RemediationStore::default();
        store.install(&mut snapshot);
        let plan = snapshot.providers[0].accounts[0]
            .remediation_plan
            .as_ref()
            .expect("remediation plan");
        assert_eq!(plan.choices.len(), 2);
        assert_eq!(plan.choices[0].kind, RemediationChoiceKind::ManagedLogin);
        assert_eq!(plan.choices[1].impact, RemediationImpact::GlobalCliIdentity);
        assert_eq!(
            plan.choices[1].command_preview.as_deref(),
            Some("claude /login")
        );
        assert!(matches!(
            store.resolve(&plan.plan_id, OPEN_TERMINAL),
            Some(AuthorizedAction::OpenTerminal {
                provider: ProviderId::Claude,
                ..
            })
        ));
    }

    #[test]
    fn rate_limits_never_offer_a_manual_fix() {
        let mut account = AccountSnapshot::failed(
            "claude:principal",
            "person@example.com",
            ProviderErrorInfo::new(ProviderErrorKind::RateLimited, "slow down"),
        );
        account.status = ProviderStatus::RateLimited;
        let mut snapshot = snapshot(ProviderId::Claude, account);
        RemediationStore::default().install(&mut snapshot);
        assert!(snapshot.providers[0].accounts[0].remediation_plan.is_none());
    }

    #[test]
    fn expired_isolated_profile_never_offers_a_global_cli_switch() {
        let account = AccountSnapshot::failed(
            "claude-profile:profile-1",
            "person@example.com",
            ProviderErrorInfo::new(ProviderErrorKind::Unauthorized, "expired"),
        );
        let mut snapshot = snapshot(ProviderId::Claude, account);
        RemediationStore::default().install(&mut snapshot);
        let plan = snapshot.providers[0].accounts[0]
            .remediation_plan
            .as_ref()
            .expect("settings plan");
        assert_eq!(plan.choices.len(), 1);
        assert_eq!(plan.choices[0].kind, RemediationChoiceKind::OpenSettings);
    }

    #[test]
    fn stale_plan_is_revoked_by_the_next_install() {
        let account = AccountSnapshot::failed(
            "codex:principal",
            "person@example.com",
            ProviderErrorInfo::new(ProviderErrorKind::CredentialsMissing, "not active"),
        );
        let mut quota_snapshot = snapshot(ProviderId::Codex, account);
        let mut store = RemediationStore::default();
        store.install(&mut quota_snapshot);
        let plan_id = quota_snapshot.providers[0].accounts[0]
            .remediation_plan
            .as_ref()
            .unwrap()
            .plan_id
            .clone();
        let mut healthy = snapshot(
            ProviderId::Codex,
            AccountSnapshot::new("codex:principal", "person@example.com"),
        );
        store.install(&mut healthy);
        assert!(store.resolve(&plan_id, MANAGED_LOGIN).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn terminal_wrapper_is_private_and_contains_only_the_vetted_command() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = std::env::temp_dir().join(format!("eyeurai-remediation-{}", random_id()));
        let script = write_unix_wrapper(
            ProviderId::Claude,
            &root,
            cli_command(ProviderId::Claude),
            "command",
        )
        .expect("write wrapper");
        let mode = fs::metadata(&script)
            .expect("wrapper metadata")
            .permissions()
            .mode()
            & 0o777;
        let content = fs::read_to_string(&script).expect("read wrapper");
        assert_eq!(mode, 0o700);
        assert!(content.contains("claude /login"));
        assert!(!content.contains("codex login"));
        fs::remove_dir_all(&root).expect("remove test wrapper directory");
    }
}
