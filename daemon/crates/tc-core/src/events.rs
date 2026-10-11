//! NDJSON event contract — port of the source's check-in progress pipeline:
//! start {type,total} / account {…} / done {type,ok,already,failed}.
//! Red line: JWT / tokens never appear in events (only sanitized fields).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Start,
    Account,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub fields: Value,
}

impl Event {
    pub fn start(total: usize) -> Event {
        Event { kind: "start".into(), fields: json!({ "total": total }) }
    }

    pub fn done(ok: usize, already: usize, failed: usize) -> Event {
        Event { kind: "done".into(), fields: json!({ "ok": ok, "already": already, "failed": failed }) }
    }

    pub fn account_success(
        index: usize,
        user_id: &str,
        name: &str,
        code: Option<i64>,
        message: &str,
        credits: Option<i64>,
        delta: Option<i64>,
    ) -> Event {
        Event {
            kind: "account".into(),
            fields: json!({
                "index": index,
                "user_id": user_id,
                "name": name,
                "status": "success",
                "code": code,
                "message": message,
                "credits": credits,
                "delta": if delta.unwrap_or(0) != 0 { delta } else { None },
            }),
        }
    }

    pub fn account_already(index: usize, user_id: &str, name: &str, credits: Option<i64>) -> Event {
        Event {
            kind: "account".into(),
            fields: json!({
                "index": index,
                "user_id": user_id,
                "name": name,
                "status": "already",
                "credits": credits,
            }),
        }
    }

    pub fn account_fail(
        index: usize,
        user_id: &str,
        name: &str,
        message: &str,
        code: Option<i64>,
        error_type: Option<&str>,
        cooldown_until: Option<i64>,
    ) -> Event {
        Event {
            kind: "account".into(),
            fields: json!({
                "index": index,
                "user_id": user_id,
                "name": name,
                "status": "fail",
                "message": message,
                "code": code,
                "error_type": error_type,
                "cooldown_until": cooldown_until,
            }),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({ "type": self.kind });
        if let Value::Object(map) = &self.fields {
            for (k, val) in map {
                v[k] = val.clone();
            }
        }
        v
    }

    pub fn to_line(&self) -> String {
        self.to_json().to_string()
    }
}

/// Progress sink — daemon writes to log file, CLI prints, tests collect.
pub trait Sink {
    fn emit(&mut self, ev: &Event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collect(Vec<Event>);
    impl Sink for Collect {
        fn emit(&mut self, ev: &Event) {
            self.0.push(ev.clone());
        }
    }

    #[test]
    fn event_pipeline_shape() {
        let mut sink = Collect::default();
        sink.emit(&Event::start(2));
        sink.emit(&Event::account_already(1, "u1", "a", Some(100)));
        sink.emit(&Event::account_success(2, "u2", "b", Some(0), "ok", Some(105), Some(5)));
        sink.emit(&Event::done(1, 1, 0));
        let lines: Vec<String> = sink.0.iter().map(|e| e.to_line()).collect();
        assert!(lines[0].contains("\"total\":2"));
        assert!(lines[1].contains("\"status\":\"already\""));
        assert!(lines[2].contains("\"delta\":5"));
        assert!(lines[3].contains("\"ok\":1"));
        // no token fields anywhere
        assert!(lines.iter().all(|l| !l.contains("jwt") && !l.contains("token")));
    }
}
