//! JSON-file persistence layer (SQLite replacement, atomic tmp+rename writes).
//!
//! Data directory layout (`/data/adb/traeclock/` on device):
//!   config.json          — AppConfig
//!   accounts.json        — AccountsFile (all apps' accounts with credentials)
//!   cooldowns.json       — CooldownMap
//!   state.json           — RunState (last round results, credits history rollup)
//!   logs/checkin.log     — human-readable summary lines
//!   events.ndjson        — machine-readable event stream (rotated)

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::error::CooldownMap;

// ── account model (union across the three apps) ─────────────────────────────

pub const APP_TRAE: &str = "trae";
pub const APP_WB: &str = "wb";
pub const APP_QODER: &str = "qoder";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    /// App id: trae | wb | qoder.
    pub app: String,
    /// Canonical user id (uid from JWT / server info).
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Group label (organizational only).
    pub group: Option<String>,
    /// Enabled accounts participate in rounds.
    pub enabled: bool,
    // ── Trae ──
    /// Trae access JWT (Cloud-IDE-JWT token, without prefix).
    pub jwt: Option<String>,
    /// Trae refresh token (OAuth).
    pub refresh_token: Option<String>,
    pub refresh_token_expires_at: Option<i64>,
    pub refresh_token_fails: i64,
    pub refresh_token_invalid: bool,
    // ── WorkBuddy / CodeBuddy ──
    pub access_token: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub domain: Option<String>,
    pub needs_relogin: bool,
    pub relogin_reason: String,
    // ── Qoder ──
    /// QoderCreds serialized (access_token / machine_id / machine_token / …).
    pub qoder_creds: Option<serde_json::Value>,
    /// Qoder device profile (stable per-account binding).
    pub qoder_device_profile: Option<serde_json::Value>,
    // ── shared lifecycle ──
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountsFile {
    pub accounts: Vec<Account>,
    pub updated_at: Option<String>,
}

// ── run state ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoundRecord {
    pub time: String,
    pub app: String,
    pub ok: usize,
    pub already: usize,
    pub failed: usize,
    /// Daily attempt counter (v0.5.25): how many rounds ran for this app
    /// today, used to cap full-failure retries. Resets each new day.
    #[serde(default)]
    pub attempts: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CreditRecord {
    pub date: String,
    pub user_id: String,
    pub credits: i64,
    pub delta: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunState {
    /// Last round per app (trae/wb/qoder).
    pub last_rounds: HashMap<String, RoundRecord>,
    /// Rolling credits history (90 days), one record per account per day.
    pub credits_history: Vec<CreditRecord>,
    /// Rolling check-in credit diffs (90 days): balance read before + after
    /// each round, latest per (date, app, user_id).
    pub credit_diffs: Vec<CreditDiff>,
}

/// One account's credit balance change across a check-in round: the balance
/// is queried right before the round and right after, and the movement is
/// recorded (positive = earned, negative = consumed meanwhile).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CreditDiff {
    /// Local date (YYYY-MM-DD).
    pub date: String,
    /// Local time of the AFTER reading (HH:MM:SS).
    pub time: String,
    pub app: String,
    pub user_id: String,
    pub name: String,
    /// Balance before the round (None if the pre-read failed).
    pub before: Option<i64>,
    /// Balance after the round (None if the post-read failed).
    pub after: Option<i64>,
    /// after − before (None when either side is missing).
    pub delta: Option<i64>,
}

impl RunState {
    /// Append a credit record; keeps the latest per (date, user_id); prunes >90 days.
    pub fn push_credit(&mut self, rec: CreditRecord) {
        self.credits_history
            .retain(|r| !(r.date == rec.date && r.user_id == rec.user_id));
        self.credits_history.push(rec);
        let cutoff = (chrono::Local::now().date_naive() - chrono::Duration::days(90))
            .format("%Y-%m-%d")
            .to_string();
        self.credits_history.retain(|r| r.date >= cutoff);
    }

    /// Append a BEFORE/AFTER credit diff record (check-in credit-change
    /// tracking). Same 90-day retention; latest wins per
    /// (date, app, user_id).
    pub fn push_credit_diff(&mut self, rec: CreditDiff) {
        self.credit_diffs
            .retain(|r| !(r.date == rec.date && r.user_id == rec.user_id && r.app == rec.app));
        self.credit_diffs.push(rec);
        let cutoff = (chrono::Local::now().date_naive() - chrono::Duration::days(90))
            .format("%Y-%m-%d")
            .to_string();
        self.credit_diffs.retain(|r| r.date >= cutoff);
    }
}

// ── store ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Store {
    pub data_dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store { data_dir: data_dir.to_path_buf() }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.data_dir.join("logs"))?;
        Ok(())
    }

    fn read_json<T: DeserializeOwned>(&self, rel: &str) -> Option<T> {
        let raw = std::fs::read_to_string(self.data_dir.join(rel)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Public typed read for helper files (pending login sessions etc.).
    pub fn read_json_opt<T: DeserializeOwned>(&self, rel: &str) -> Option<T> {
        self.read_json::<T>(rel)
    }

    fn write_json(&self, rel: &str, value: &impl Serialize) -> Result<(), String> {
        let path = self.data_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn config(&self) -> AppConfig {
        self.read_json::<AppConfig>("config.json").unwrap_or_default().sanitized()
    }

    pub fn save_config(&self, cfg: &AppConfig) -> Result<(), String> {
        self.write_json("config.json", cfg)
    }

    /// Persist an arbitrary helper JSON file (pending login sessions etc.).
    pub fn save_json(&self, rel: &str, value: &impl Serialize) -> Result<(), String> {
        self.write_json(rel, value)
    }

    pub fn accounts(&self) -> AccountsFile {
        self.read_json::<AccountsFile>("accounts.json").unwrap_or_default()
    }

    pub fn save_accounts(&self, acc: &AccountsFile) -> Result<(), String> {
        let mut acc = acc.clone();
        acc.updated_at = Some(crate::util::now_iso_local());
        self.write_json("accounts.json", &acc)
    }

    pub fn cooldowns(&self) -> CooldownMap {
        self.read_json::<CooldownMap>("cooldowns.json").unwrap_or_default()
    }

    pub fn save_cooldowns(&self, cd: &CooldownMap) -> Result<(), String> {
        self.write_json("cooldowns.json", cd)
    }

    pub fn state(&self) -> RunState {
        self.read_json::<RunState>("state.json").unwrap_or_default()
    }

    pub fn save_state(&self, st: &RunState) -> Result<(), String> {
        self.write_json("state.json", st)
    }

    /// Append a summary line to logs/checkin.log.
    pub fn append_checkin_log(&self, line: &str) {
        let path = self.data_dir.join("logs").join("checkin.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Append an NDJSON event line to logs/events.ndjson (with size rotation ~2MB).
    pub fn append_event(&self, line: &str) {
        let path = self.data_dir.join("logs").join("events.ndjson");
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 2_000_000 {
                let _ = std::fs::rename(&path, self.data_dir.join("logs").join("events.ndjson.1"));
            }
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Read the last N lines of logs/events.ndjson (for the manager app).
    pub fn tail_events(&self, n: usize) -> Vec<String> {
        let path = self.data_dir.join("logs").join("events.ndjson");
        let Ok(raw) = std::fs::read_to_string(&path) else { return Vec::new() };
        let lines: Vec<&str> = raw.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].iter().map(|s| s.to_string()).collect()
    }

    /// Clean up log files older than days days (manager 清理日志):
    /// - logs/checkin.log: keep only lines dated within the window (parse the
    ///   leading "[YYYY-MM-DD ...]" timestamp; undated legacy lines are kept),
    ///   rewriting the file in place
    /// - logs/events.ndjson + rotated events.ndjson.1: NDJSON events carry
    ///   "ts" - files whose every line is older than the window are dropped
    ///   outright; the current file is filtered line-by-line otherwise
    ///
    /// Returns (removed_files, kept_lines) for the confirmation toast.
    /// Never touches anything outside the logs/ directory.
    pub fn clean_logs_older_than(&self, days: u32) -> Result<(usize, usize), String> {
        use chrono::NaiveDate;
        let logs = self.data_dir.join("logs");
        let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(days as i64);
        let mut removed_files = 0usize;
        let mut kept_lines = 0usize;

        // 1. checkin.log - filter by leading [YYYY-MM-DD timestamp
        let cl = logs.join("checkin.log");
        if let Ok(raw) = std::fs::read_to_string(&cl) {
            let mut kept: Vec<&str> = Vec::new();
            for line in raw.lines() {
                let dated = line
                    .strip_prefix('[')
                    .and_then(|rest| rest.get(..10))
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
                match dated {
                    Some(d) if d < cutoff => {} // drop: older than the window
                    _ => {
                        kept.push(line);
                        kept_lines += 1;
                    }
                }
            }
            let body = if kept.is_empty() { String::new() } else { kept.join("
") + "
" };
            std::fs::write(&cl, body).map_err(|e| e.to_string())?;
        }

        // 2. events.ndjson(.1) - drop the file when every dated line is stale,
        //    else rewrite with only fresh lines; undated lines are kept.
        for name in ["events.ndjson", "events.ndjson.1"] {
            let p = logs.join(name);
            let Ok(raw) = std::fs::read_to_string(&p) else { continue };
            let lines: Vec<&str> = raw.lines().collect();
            let mut fresh: Vec<&str> = Vec::new();
            for l in &lines {
                let keep = serde_json::from_str::<serde_json::Value>(l)
                    .ok()
                    .and_then(|v| v.get("ts").and_then(|t| t.as_str()).map(String::from))
                    .and_then(|ts| NaiveDate::parse_from_str(&ts[..10.min(ts.len())], "%Y-%m-%d").ok())
                    .map(|d| d >= cutoff)
                    .unwrap_or(true); // no ts / unparsable -> keep
                if keep {
                    fresh.push(*l);
                }
            }
            if fresh.is_empty() && !lines.is_empty() {
                let _ = std::fs::remove_file(&p);
                removed_files += 1;
            } else if fresh.len() < lines.len() {
                let body: String = fresh.join("
") + "
";
                std::fs::write(&p, body).map_err(|e| e.to_string())?;
                kept_lines += fresh.len();
            } else {
                kept_lines += fresh.len();
            }
        }
        Ok((removed_files, kept_lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::fingerprint::DeviceEntry;

    fn tmpdir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!(
            "tc-core-test-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis(),
            n
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_config_accounts_state() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        st.ensure_dirs().unwrap();

        let mut cfg = AppConfig::default();
        cfg.header_overrides.insert("trae:x-test".into(), "v".into());
        st.save_config(&cfg).unwrap();
        assert_eq!(st.config().header_overrides["trae:x-test"], "v");

        let mut acc = AccountsFile::default();
        acc.accounts.push(Account {
            app: APP_TRAE.into(),
            user_id: "u1".into(),
            name: "n1".into(),
            enabled: true,
            jwt: Some("abc".into()),
            ..Default::default()
        });
        st.save_accounts(&acc).unwrap();
        let loaded = st.accounts();
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].jwt.as_deref(), Some("abc"));
        assert!(loaded.updated_at.is_some());

        let mut state = RunState::default();
        state.push_credit(CreditRecord {
            date: "2026-10-11".into(),
            user_id: "u1".into(),
            credits: 100,
            delta: 5,
        });
        state.push_credit(CreditRecord {
            date: "2026-10-11".into(),
            user_id: "u1".into(),
            credits: 105,
            delta: 5,
        });
        st.save_state(&state).unwrap();
        assert_eq!(st.state().credits_history.len(), 1); // same-day dedup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn device_map_persists_and_wins_over_derivation() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        let mut cfg = AppConfig::default();
        cfg.device_map.insert(
            "u9".into(),
            DeviceEntry { device_id: "123456789012345".into(), ..Default::default() },
        );
        st.save_config(&cfg).unwrap();
        let cfg2 = st.config();
        assert_eq!(cfg2.device_map["u9"].device_id, "123456789012345");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn events_rotate_and_tail() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        st.ensure_dirs().unwrap();
        for i in 0..50 {
            st.append_event(&format!("{{\"i\":{i}}}"));
        }
        let tail = st.tail_events(5);
        assert_eq!(tail.len(), 5);
        assert!(tail[4].contains("\"i\":49"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// clean_logs_older_than: checkin.log lines dated older than the window
    /// are dropped (undated legacy lines kept); events.ndjson lines with a
    /// fresh ts survive, all-stale files are removed entirely.
    #[test]
    fn clean_logs_drops_old_keeps_fresh() {
        let dir = tmpdir();
        let st = Store::new(&dir);
        st.ensure_dirs().unwrap();
        let old = (chrono::Local::now().date_naive() - chrono::Duration::days(10))
            .format("%Y-%m-%d").to_string();
        let fresh = (chrono::Local::now().date_naive() - chrono::Duration::days(1))
            .format("%Y-%m-%d").to_string();
        // checkin.log: old dated + fresh dated + undated legacy line
        let cl = dir.join("logs").join("checkin.log");
        std::fs::write(&cl, format!(
            "[{old} 08:00:00] [trae] 成功0/已签到0/失败1/总计1\n[{fresh} 09:00:00] [trae] 成功1/总计1\nlegacy undated line\n"
        )).unwrap();
        // events.ndjson: one stale (10d old ts) + one fresh line
        let ev = dir.join("logs").join("events.ndjson");
        std::fs::write(&ev, format!(
            "{{\"type\":\"account\",\"ts\":\"{old}T08:00:00\"}}\n{{\"type\":\"done\",\"ts\":\"{fresh}T09:00:00\"}}\n"
        )).unwrap();
        // rotated file entirely stale → should be removed
        let ev1 = dir.join("logs").join("events.ndjson.1");
        std::fs::write(&ev1, format!("{{\"type\":\"done\",\"ts\":\"{old}T08:00:00\"}}\n")).unwrap();

        let (removed_files, kept_lines) = st.clean_logs_older_than(7).unwrap();

        let after = std::fs::read_to_string(&cl).unwrap();
        assert!(!after.contains(&old), "old checkin line dropped");
        assert!(after.contains(&fresh), "fresh checkin line kept");
        assert!(after.contains("legacy undated line"), "undated line kept");
        let ev_after = std::fs::read_to_string(&ev).unwrap();
        assert!(ev_after.contains("\"done\""), "fresh event kept");
        assert!(!ev_after.contains("account"), "stale event dropped");
        assert!(!ev1.exists(), "fully stale rotated file removed");
        assert_eq!(removed_files, 1);
        assert!(kept_lines >= 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
