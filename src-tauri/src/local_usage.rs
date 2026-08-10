//! Read-only token counters from local Claude Code and Codex CLI session logs.
//!
//! The scanner returns aggregate numbers and model labels only. Paths, message
//! text, session identifiers, and credentials never cross the Tauri boundary.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::Value;

const DEFAULT_RANGE_DAYS: u32 = 7;
const MAX_RANGE_DAYS: u32 = 90;
const MAX_FILES: usize = 8_000;
const MAX_FILE_BYTES: u64 = 96 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 768 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 12 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageProviderSummary {
    pub provider: String,
    pub processed_tokens: u64,
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub observations: u64,
    pub sessions: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageDaySummary {
    pub date: String,
    pub provider: String,
    pub processed_tokens: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageModelSummary {
    pub provider: String,
    pub model: String,
    pub processed_tokens: u64,
}

impl LocalUsageProviderSummary {
    fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            ..Self::default()
        }
    }

    fn add(&mut self, usage: UsageCounters) {
        self.processed_tokens = self.processed_tokens.saturating_add(usage.processed);
        self.uncached_input_tokens = self
            .uncached_input_tokens
            .saturating_add(usage.uncached_input);
        self.cached_input_tokens = self.cached_input_tokens.saturating_add(usage.cached_input);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(usage.cache_write_input);
        self.output_tokens = self.output_tokens.saturating_add(usage.output);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(usage.reasoning_output);
        self.observations = self.observations.saturating_add(1);
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageSnapshot {
    pub generated_at: DateTime<Utc>,
    pub range_days: u32,
    pub processed_tokens: u64,
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub observations: u64,
    pub sessions: u64,
    pub providers: Vec<LocalUsageProviderSummary>,
    pub daily: Vec<LocalUsageDaySummary>,
    pub models: Vec<LocalUsageModelSummary>,
}

impl LocalUsageSnapshot {
    fn from_scans(range_days: u32, scans: Vec<ProviderScan>) -> Self {
        let providers: Vec<_> = scans.iter().map(|scan| scan.summary.clone()).collect();
        let sum = |field: fn(&LocalUsageProviderSummary) -> u64| {
            providers.iter().map(field).fold(0_u64, u64::saturating_add)
        };
        let mut daily: Vec<_> = scans
            .iter()
            .flat_map(|scan| {
                scan.daily
                    .iter()
                    .map(|(date, processed_tokens)| LocalUsageDaySummary {
                        date: date.clone(),
                        provider: scan.summary.provider.clone(),
                        processed_tokens: *processed_tokens,
                    })
            })
            .collect();
        daily.sort_by(|left, right| {
            left.date
                .cmp(&right.date)
                .then_with(|| left.provider.cmp(&right.provider))
        });
        let mut models: Vec<_> = scans
            .iter()
            .flat_map(|scan| {
                scan.models
                    .iter()
                    .map(|(model, processed_tokens)| LocalUsageModelSummary {
                        provider: scan.summary.provider.clone(),
                        model: model.clone(),
                        processed_tokens: *processed_tokens,
                    })
            })
            .collect();
        models.sort_by(|left, right| {
            right
                .processed_tokens
                .cmp(&left.processed_tokens)
                .then_with(|| left.model.cmp(&right.model))
        });
        models.truncate(40);
        Self {
            generated_at: Utc::now(),
            range_days,
            processed_tokens: sum(|row| row.processed_tokens),
            uncached_input_tokens: sum(|row| row.uncached_input_tokens),
            cached_input_tokens: sum(|row| row.cached_input_tokens),
            cache_write_input_tokens: sum(|row| row.cache_write_input_tokens),
            output_tokens: sum(|row| row.output_tokens),
            reasoning_output_tokens: sum(|row| row.reasoning_output_tokens),
            observations: sum(|row| row.observations),
            sessions: sum(|row| row.sessions),
            providers,
            daily,
            models,
        }
    }
}

#[derive(Default)]
struct ProviderScan {
    summary: LocalUsageProviderSummary,
    daily: HashMap<String, u64>,
    models: HashMap<String, u64>,
}

impl ProviderScan {
    fn new(provider: &str) -> Self {
        Self {
            summary: LocalUsageProviderSummary::new(provider),
            ..Self::default()
        }
    }

    fn add(&mut self, timestamp: &str, model: &str, usage: UsageCounters) {
        self.summary.add(usage);
        if let Some(date) = timestamp_date(timestamp) {
            let total = self.daily.entry(date).or_default();
            *total = total.saturating_add(usage.processed);
        }
        let model = safe_model_label(model, &self.summary.provider);
        let total = self.models.entry(model).or_default();
        *total = total.saturating_add(usage.processed);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct UsageCounters {
    processed: u64,
    uncached_input: u64,
    cached_input: u64,
    cache_write_input: u64,
    output: u64,
    reasoning_output: u64,
}

#[derive(Default)]
struct ScanBudget {
    files: usize,
    bytes: u64,
}

#[tauri::command]
pub async fn scan_local_usage(days: Option<u32>) -> Result<LocalUsageSnapshot, String> {
    let range_days = days.unwrap_or(DEFAULT_RANGE_DAYS).clamp(1, MAX_RANGE_DAYS);
    tauri::async_runtime::spawn_blocking(move || scan(range_days))
        .await
        .map_err(|_| "the local usage scan did not finish".to_string())
}

fn scan(range_days: u32) -> LocalUsageSnapshot {
    let today = Utc::now().date_naive();
    let first_day = today - Duration::days(i64::from(range_days.saturating_sub(1)));
    let since = DateTime::<Utc>::from_naive_utc_and_offset(
        first_day
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always a valid time"),
        Utc,
    );
    let home = match dirs::home_dir() {
        Some(path) => path,
        None => return LocalUsageSnapshot::from_scans(range_days, Vec::new()),
    };
    let mut budget = ScanBudget::default();
    let mut scans = Vec::new();

    let claude = scan_claude(&home.join(".claude/projects"), since, &mut budget);
    if claude.summary.observations > 0 {
        scans.push(claude);
    }

    let codex = scan_codex(&home.join(".codex/sessions"), since, &mut budget);
    if codex.summary.observations > 0 {
        scans.push(codex);
    }

    LocalUsageSnapshot::from_scans(range_days, scans)
}

fn scan_claude(root: &Path, since: DateTime<Utc>, budget: &mut ScanBudget) -> ProviderScan {
    let mut scan = ProviderScan::new("claude");
    let mut seen = HashSet::new();
    for path in recent_jsonl_files(root, since, budget) {
        let before = scan.summary.observations;
        visit_json_lines(&path, budget, |value| {
            if !timestamp_in_range(value, since) {
                return;
            }
            let message = match value.get("message") {
                Some(message)
                    if message.get("role").and_then(Value::as_str) == Some("assistant") =>
                {
                    message
                }
                _ => return,
            };
            let usage = match message.get("usage") {
                Some(usage) => usage,
                None => return,
            };
            if let Some(id) = message
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                if !seen.insert(id.to_string()) {
                    return;
                }
            }
            if let Some(counters) = claude_counters(usage) {
                let timestamp = value.get("timestamp").and_then(Value::as_str).unwrap_or("");
                let model = message
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("Claude");
                scan.add(timestamp, model, counters);
            }
        });
        if scan.summary.observations > before {
            scan.summary.sessions = scan.summary.sessions.saturating_add(1);
        }
    }
    scan
}

fn scan_codex(root: &Path, since: DateTime<Utc>, budget: &mut ScanBudget) -> ProviderScan {
    let mut scan = ProviderScan::new("openai");
    for path in recent_jsonl_files(root, since, budget) {
        let before = scan.summary.observations;
        let mut current_model = String::from("Codex");
        let mut seen = HashSet::new();
        visit_json_lines(&path, budget, |value| {
            if value.get("type").and_then(Value::as_str) == Some("turn_context") {
                if let Some(model) = value
                    .get("payload")
                    .and_then(|payload| payload.get("model"))
                    .and_then(Value::as_str)
                    .filter(|model| !model.is_empty())
                {
                    current_model = model.to_string();
                }
                return;
            }
            if !timestamp_in_range(value, since) {
                return;
            }
            let payload = match value.get("payload") {
                Some(payload)
                    if payload.get("type").and_then(Value::as_str) == Some("token_count") =>
                {
                    payload
                }
                _ => return,
            };
            let usage = match payload
                .get("info")
                .and_then(|info| info.get("last_token_usage"))
            {
                Some(usage) => usage,
                None => return,
            };
            let counters = match codex_counters(usage) {
                Some(counters) => counters,
                None => return,
            };
            let timestamp = value.get("timestamp").and_then(Value::as_str).unwrap_or("");
            let key = format!(
                "{timestamp}:{}:{}:{}:{}:{}",
                counters.processed,
                counters.uncached_input,
                counters.cached_input,
                counters.output,
                counters.reasoning_output
            );
            if seen.insert(key) {
                scan.add(timestamp, &current_model, counters);
            }
        });
        if scan.summary.observations > before {
            scan.summary.sessions = scan.summary.sessions.saturating_add(1);
        }
    }
    scan
}

fn claude_counters(usage: &Value) -> Option<UsageCounters> {
    let input = number(usage, "input_tokens");
    let cached = number(usage, "cache_read_input_tokens");
    let cache_write = number(usage, "cache_creation_input_tokens");
    let output = number(usage, "output_tokens");
    let processed = input
        .saturating_add(cached)
        .saturating_add(cache_write)
        .saturating_add(output);
    (processed > 0).then_some(UsageCounters {
        processed,
        uncached_input: input,
        cached_input: cached,
        cache_write_input: cache_write,
        output,
        reasoning_output: 0,
    })
}

fn codex_counters(usage: &Value) -> Option<UsageCounters> {
    let input = number(usage, "input_tokens");
    let cached = number(usage, "cached_input_tokens");
    let cache_write = number(usage, "cache_write_input_tokens");
    let output = number(usage, "output_tokens");
    let reasoning = number(usage, "reasoning_output_tokens").min(output);
    let uncached = input.saturating_sub(cached.saturating_add(cache_write));
    let processed = input.saturating_add(output);
    (processed > 0).then_some(UsageCounters {
        processed,
        uncached_input: uncached,
        cached_input: cached.min(input),
        cache_write_input: cache_write.min(input),
        output,
        reasoning_output: reasoning,
    })
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn timestamp_date(timestamp: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|value| value.date_naive().format("%Y-%m-%d").to_string())
}

fn safe_model_label(raw: &str, provider: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "<synthetic>" {
        return if provider == "claude" {
            "Claude"
        } else {
            "Codex"
        }
        .to_string();
    }
    trimmed.chars().take(80).collect()
}

fn timestamp_in_range(value: &Value, since: DateTime<Utc>) -> bool {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc) >= since)
        .unwrap_or(false)
}

fn recent_jsonl_files(root: &Path, since: DateTime<Utc>, budget: &ScanBudget) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let since_system: SystemTime = since.into();
    collect_jsonl(root, since_system, &mut files, 0);
    files.sort_by(|left, right| {
        modified(right)
            .cmp(&modified(left))
            .then_with(|| left.cmp(right))
    });
    files.truncate(MAX_FILES.saturating_sub(budget.files));
    files
}

fn collect_jsonl(root: &Path, since: SystemTime, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 || files.len() >= MAX_FILES {
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            collect_jsonl(&path, since, files, depth + 1);
        } else if metadata.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && metadata.len() <= MAX_FILE_BYTES
            && metadata
                .modified()
                .map(|time| time >= since)
                .unwrap_or(false)
        {
            files.push(path);
        }
        if files.len() >= MAX_FILES {
            return;
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn visit_json_lines(path: &Path, budget: &mut ScanBudget, mut visit: impl FnMut(&Value)) {
    if budget.files >= MAX_FILES || budget.bytes >= MAX_TOTAL_BYTES {
        return;
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return,
    };
    budget.files += 1;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let read = match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        budget.bytes = budget.bytes.saturating_add(read as u64);
        if budget.bytes > MAX_TOTAL_BYTES {
            break;
        }
        if read > MAX_LINE_BYTES {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            visit(&value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_arithmetic_keeps_cache_categories_separate() {
        let value: Value = serde_json::from_str(
            r#"{"input_tokens":2,"cache_creation_input_tokens":30,"cache_read_input_tokens":40,"output_tokens":5}"#,
        )
        .unwrap();
        assert_eq!(
            claude_counters(&value),
            Some(UsageCounters {
                processed: 77,
                uncached_input: 2,
                cached_input: 40,
                cache_write_input: 30,
                output: 5,
                reasoning_output: 0,
            })
        );
    }

    #[test]
    fn codex_arithmetic_treats_cached_and_reasoning_as_subsets() {
        let value: Value = serde_json::from_str(
            r#"{"input_tokens":100,"cached_input_tokens":60,"cache_write_input_tokens":10,"output_tokens":20,"reasoning_output_tokens":8}"#,
        )
        .unwrap();
        assert_eq!(
            codex_counters(&value),
            Some(UsageCounters {
                processed: 120,
                uncached_input: 30,
                cached_input: 60,
                cache_write_input: 10,
                output: 20,
                reasoning_output: 8,
            })
        );
    }

    #[test]
    fn snapshot_sums_provider_rows_without_exposing_detail() {
        let mut claude = ProviderScan::new("claude");
        claude.add(
            "2026-08-09T12:00:00Z",
            "claude-opus",
            UsageCounters {
                processed: 10,
                uncached_input: 2,
                cached_input: 4,
                cache_write_input: 1,
                output: 3,
                reasoning_output: 0,
            },
        );
        let snapshot = LocalUsageSnapshot::from_scans(7, vec![claude]);
        assert_eq!(snapshot.processed_tokens, 10);
        assert_eq!(snapshot.observations, 1);
        assert_eq!(snapshot.daily[0].processed_tokens, 10);
        assert_eq!(snapshot.models[0].model, "claude-opus");
        let json = serde_json::to_value(snapshot).unwrap();
        assert!(json.get("processedTokens").is_some());
        assert!(json.get("paths").is_none());
    }
}
