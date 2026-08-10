//! Deterministic sample data used by tests, screenshots, and frontend preview.

use chrono::{DateTime, Duration, Utc};

use crate::models::{
    AccountSnapshot, DataSource, Freshness, Measure, ProviderSnapshot, QuotaSnapshot, QuotaWindow,
    Unit, WindowKind,
};

use super::{claude::ClaudeProvider, codex::CodexProvider, openrouter::OpenRouterProvider};

pub fn snapshot(now: DateTime<Utc>) -> QuotaSnapshot {
    let claude = account(
        "claude-demo",
        "dev@example.com",
        "Max",
        vec![
            QuotaWindow::new("five_hour", "5-hour session", WindowKind::Rolling)
                .with_used_percent(68.0)
                .with_reset(now + Duration::hours(2) + Duration::minutes(14), now)
                .with_window_seconds(18_000),
            QuotaWindow::new("seven_day", "Weekly", WindowKind::Rolling)
                .with_used_percent(42.0)
                .with_reset(now + Duration::days(4) + Duration::hours(7), now)
                .with_window_seconds(604_800),
        ],
        now,
    );
    let codex = account(
        "codex-demo",
        "Personal",
        "Plus",
        vec![
            QuotaWindow::new("five_hour", "5-hour session", WindowKind::Rolling)
                .with_used_percent(31.0)
                .with_reset(now + Duration::hours(3) + Duration::minutes(9), now)
                .with_window_seconds(18_000),
            QuotaWindow::new("seven_day", "Weekly", WindowKind::Rolling)
                .with_used_percent(54.0)
                .with_reset(now + Duration::days(5), now)
                .with_window_seconds(604_800),
        ],
        now,
    );
    let openrouter = account(
        "openrouter-demo",
        "Work API key",
        "API key",
        vec![
            QuotaWindow::new("spend_limit", "Monthly spend limit", WindowKind::Calendar)
                .with_used(Measure::usd(18.40))
                .with_limit(Measure::new(50.0, Unit::Usd))
                .derive_percent_from_measures()
                .with_reset(now + Duration::days(12), now),
        ],
        now,
    );

    let mut claude_provider =
        ProviderSnapshot::new(ClaudeProvider::capability_info()).with_accounts(vec![claude]);
    claude_provider.freshness = Freshness::demo(now);
    let mut codex_provider =
        ProviderSnapshot::new(CodexProvider::capability_info()).with_accounts(vec![codex]);
    codex_provider.freshness = Freshness::demo(now);
    let mut openrouter_provider = ProviderSnapshot::new(OpenRouterProvider::capability_info())
        .with_accounts(vec![openrouter]);
    openrouter_provider.freshness = Freshness::demo(now);

    QuotaSnapshot::new(
        now,
        DataSource::Demo,
        vec![claude_provider, codex_provider, openrouter_provider],
    )
}

fn account(
    id: &str,
    label: &str,
    plan: &str,
    windows: Vec<QuotaWindow>,
    now: DateTime<Utc>,
) -> AccountSnapshot {
    let mut account = AccountSnapshot::new(id, label);
    account.plan = Some(plan.to_string());
    account.active = true;
    account.windows = windows;
    account.freshness = Freshness::demo(now);
    account
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_secret_free_and_has_the_core_windows() {
        let now = Utc::now();
        let snapshot = snapshot(now);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("five_hour"));
        assert!(json.contains("seven_day"));
        assert!(!json.contains("sk-"));
    }
}
