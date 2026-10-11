//! Shared HTTP layer: direct-connect agents (ureq never reads system/env
//! proxies — same as the source's "NO_PROXY=*" convention) and small JSON
//! POST/GET helpers returning (status, parsed, raw).
//!
//! status == 0 means transport-level failure (timeout / connection error),
//! mirroring the source's python-inherited convention.

use serde_json::Value;

/// Blocking agent with a total timeout. Direct-connect semantics.
pub fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .max_idle_connections(8)
        .max_idle_connections_per_host(8)
        .build()
}

/// POST JSON with headers → (http_status, parsed, raw_text).
pub fn post_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
    body: &Value,
) -> (i32, Option<Value>, String) {
    let mut req = agent.post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.send_string(&body.to_string()) {
        Ok(resp) => {
            let raw = resp.into_string().unwrap_or_default();
            let parsed = serde_json::from_str(&raw).ok();
            (200, parsed, raw)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let raw = resp.into_string().unwrap_or_else(|e| format!("<read failed: {e}>"));
            let parsed = serde_json::from_str(&raw).ok();
            (code as i32, parsed, raw)
        }
        Err(e) => (0, None, e.to_string()),
    }
}

/// GET with headers → (http_status, parsed, raw_text).
pub fn get_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
) -> (i32, Option<Value>, String) {
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.call() {
        Ok(resp) => {
            let raw = resp.into_string().unwrap_or_default();
            let parsed = serde_json::from_str(&raw).ok();
            (200, parsed, raw)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let raw = resp.into_string().unwrap_or_else(|e| format!("<read failed: {e}>"));
            let parsed = serde_json::from_str(&raw).ok();
            (code as i32, parsed, raw)
        }
        Err(e) => (0, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_builds() {
        // no network involved — just ensure construction is sane
        let _a = agent(5);
    }
}
