//! Round orchestration across the three apps: runs each enabled app's round
//! against the shared store, recording last_rounds so the scheduler can decide
//! "ran today".
//!
//! Credit-change tracking: for every round, each enabled account's balance is
//! probed right BEFORE the round and right AFTER (per-app credit endpoints).
//! The movement (after − before) is persisted to state.credit_diffs (90-day
//! rolling) and emitted as NDJSON events so the manager log page shows it.
//! A failed probe on either side leaves that side None — never recorded as 0.

use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::credits;
use crate::events::{Event, Sink};
use crate::store::{CreditDiff, RoundRecord, RunState, Store, APP_QODER, APP_TRAE, APP_WB};

/// Run one round for every enabled app (used by `run-now` and the daemon).
pub fn run_all(store: &Store, cfg: &AppConfig, sink: &mut dyn Sink) -> Value {
    let mut out = serde_json::Map::new();
    if cfg.enable_trae {
        out.insert(APP_TRAE.to_string(), run_one(store, cfg, APP_TRAE, sink).unwrap_or(Value::Null));
    }
    if cfg.enable_wb {
        out.insert(APP_WB.to_string(), run_one(store, cfg, APP_WB, sink).unwrap_or(Value::Null));
    }
    if cfg.enable_qoder {
        out.insert(APP_QODER.to_string(), run_one(store, cfg, APP_QODER, sink).unwrap_or(Value::Null));
    }
    Value::Object(out)
}

/// Run a single app's round by app id; records the round in state.last_rounds
/// (the daemon's "ran today" gate reads this). Wraps the round with
/// before/after credit probes per enabled account and persists the diffs.
pub fn run_one(store: &Store, cfg: &AppConfig, app: &str, sink: &mut dyn Sink) -> Option<Value> {
    // pre/post round balance snapshot per enabled account (credit endpoints)
    let snapshot_accounts = || -> Vec<(crate::store::Account, Option<i64>)> {
        if !matches!(app, APP_TRAE | APP_WB | APP_QODER) {
            return Vec::new();
        }
        let agent = crate::http::agent(30);
        store
            .accounts()
            .accounts
            .iter()
            .filter(|a| a.app == app && a.enabled)
            .map(|a| (a.clone(), credits::account_balance(&agent, cfg, a)))
            .collect()
    };

    let pre = snapshot_accounts();
    let v = match app {
        APP_TRAE if cfg.enable_trae => crate::trae::run_round(store, cfg, sink),
        APP_WB if cfg.enable_wb => crate::workbuddy::run_round(store, cfg, sink),
        APP_QODER if cfg.enable_qoder => crate::qoder::run_round(store, cfg, sink),
        _ => return None,
    };
    let post = snapshot_accounts();
    record_credit_diffs(store, app, &pre, &post, sink);
    record_round(store, app, &v);
    Some(v)
}

/// Compare pre/post snapshots, persist one CreditDiff per account and emit a
/// log event per account with a known delta.
fn record_credit_diffs(
    store: &Store,
    app: &str,
    pre: &[(crate::store::Account, Option<i64>)],
    post: &[(crate::store::Account, Option<i64>)],
    sink: &mut dyn Sink,
) {
    if pre.is_empty() && post.is_empty() {
        return;
    }
    let mut state: RunState = store.state();
    let mut dirty = false;
    for (acc, before) in pre {
        let after = post
            .iter()
            .find(|(a, _)| a.user_id == acc.user_id)
            .and_then(|(_, b)| *b);
        let delta = match (before, after) {
            (Some(b), Some(a)) => Some(a - b),
            _ => None,
        };
        let diff = CreditDiff {
            date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            app: app.to_string(),
            user_id: acc.user_id.clone(),
            name: if acc.name.is_empty() { acc.user_id.clone() } else { acc.name.clone() },
            before: *before,
            after,
            delta,
        };
        state.push_credit_diff(diff.clone());
        dirty = true;
        if let Some(d) = delta {
            // event: [ts] [credits_diff] app=trae user_id=u name=n before=100 after=120 delta=20
            sink.emit(&Event {
                kind: "credits_diff".into(),
                fields: json!({
                    "app": app,
                    "user_id": acc.user_id,
                    "name": diff.name,
                    "before": before,
                    "after": after,
                    "delta": d,
                }),
            });
            store.append_event(&Event {
                kind: "credits_diff".into(),
                fields: json!({
                    "app": app,
                    "user_id": acc.user_id,
                    "name": diff.name,
                    "before": before,
                    "after": after,
                    "delta": d,
                }),
            }.to_line());
        }
    }
    if dirty {
        let _ = store.save_state(&state);
    }
}

fn record_round(store: &Store, app: &str, done: &Value) {
    let mut state: RunState = store.state();
    let rec = RoundRecord {
        time: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        app: app.to_string(),
        ok: done.get("ok").and_then(|x| x.as_i64()).unwrap_or(0) as usize,
        already: done.get("already").and_then(|x| x.as_i64()).unwrap_or(0) as usize,
        failed: done.get("failed").and_then(|x| x.as_i64()).unwrap_or(0) as usize,
    };
    state.last_rounds.insert(app.to_string(), rec);
    let _ = store.save_state(&state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Sink;
    use crate::store::Account;

    #[derive(Default)]
    struct Collect(Vec<Event>);
    impl Sink for Collect {
        fn emit(&mut self, ev: &Event) {
            self.0.push(ev.clone());
        }
    }

    fn tmpdir() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!(
            "tc-runner-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn acc(uid: &str) -> Account {
        Account {
            app: APP_TRAE.into(),
            user_id: uid.into(),
            name: format!("acct-{uid}"),
            enabled: true,
            ..Default::default()
        }
    }

    /// Diff computation: positive movement recorded + event emitted; missing
    /// probe side → no delta, no event; account only in post → skipped.
    #[test]
    fn credit_diffs_recorded_and_emitted() {
        let dir = tmpdir();
        let store = Store::new(&dir);
        let mut sink = Collect::default();

        let pre = vec![(acc("u1"), Some(1000)), (acc("u2"), Some(500)), (acc("u3"), None)];
        // u1: +20 earned; u2: consumed 50 (negative delta still recorded);
        // u3: post failed → no delta; u4: only in post → skipped
        let post = vec![
            (acc("u1"), Some(1020)),
            (acc("u2"), Some(450)),
            (acc("u3"), None),
            (acc("u4"), Some(9)),
        ];
        record_credit_diffs(&store, APP_TRAE, &pre, &post, &mut sink);

        let state = store.state();
        assert_eq!(state.credit_diffs.len(), 3, "u4 (post-only) must be skipped");
        let by_uid = |u: &str| state.credit_diffs.iter().find(|d| d.user_id == u).unwrap();
        assert_eq!(by_uid("u1").delta, Some(20));
        assert_eq!(by_uid("u1").before, Some(1000));
        assert_eq!(by_uid("u1").after, Some(1020));
        assert_eq!(by_uid("u2").delta, Some(-50));
        assert_eq!(by_uid("u3").delta, None);
        // events only for accounts with a computable delta
        let evs: Vec<&Event> = sink.0.iter().filter(|e| e.kind == "credits_diff").collect();
        assert_eq!(evs.len(), 2, "u3 (missing side) emits no event");
        assert!(sink.0.iter().all(|e| e.to_line().contains("\"delta\":20")
            || e.to_line().contains("\"delta\":-50")
            || e.kind != "credits_diff"));
        // overwrite semantics: same (date, app, uid) keeps only the latest
        let pre2 = vec![(acc("u1"), Some(1020))];
        let post2 = vec![(acc("u1"), Some(1035))];
        record_credit_diffs(&store, APP_TRAE, &pre2, &post2, &mut sink);
        let state2 = store.state();
        assert_eq!(state2.credit_diffs.len(), 3);
        assert_eq!(
            state2.credit_diffs.iter().find(|d| d.user_id == "u1").unwrap().delta,
            Some(15)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
