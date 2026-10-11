//! Round orchestration across the three apps: runs each enabled app's round
//! against the shared store, recording last_rounds so the scheduler can decide
//! "ran today".

use serde_json::Value;

use crate::config::AppConfig;
use crate::events::Sink;
use crate::store::{RoundRecord, RunState, Store, APP_QODER, APP_TRAE, APP_WB};

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
/// (the daemon's "ran today" gate reads this).
pub fn run_one(store: &Store, cfg: &AppConfig, app: &str, sink: &mut dyn Sink) -> Option<Value> {
    let v = match app {
        APP_TRAE if cfg.enable_trae => crate::trae::run_round(store, cfg, sink),
        APP_WB if cfg.enable_wb => crate::workbuddy::run_round(store, cfg, sink),
        APP_QODER if cfg.enable_qoder => crate::qoder::run_round(store, cfg, sink),
        _ => return None,
    };
    record_round(store, app, &v);
    Some(v)
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
