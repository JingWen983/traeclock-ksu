//! Shared HTTP layer: direct-connect agents (ureq never reads system/env
//! proxies — same as the source's "NO_PROXY=*" convention) and small JSON
//! POST/GET helpers returning (status, parsed, raw).
//!
//! status == 0 means transport-level failure (timeout / connection error),
//! mirroring the source's python-inherited convention.

use serde_json::Value;

/// 传输层错误 → 用户可读文案（移植自上游 friendly_net_err，issue #209）。
/// 原文保留在括号里方便排障；不认识的错误原样返回，避免吞掉诊断信息。
pub fn friendly_net_err(e: &str) -> String {
    let lower = e.to_lowercase();
    let hint = if lower.contains("dns") || lower.contains("resolve") || lower.contains("no such host") {
        "DNS 解析失败"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "连接超时"
    } else if lower.contains("refused") {
        "连接被拒绝"
    } else if lower.contains("reset") {
        "连接被重置"
    } else if lower.contains("certificate") || lower.contains("tls") {
        "TLS 握手失败"
    } else if lower.contains("unreachable") {
        "网络不可达"
    } else {
        return e.to_string();
    };
    format!("{hint}（{e}）")
}

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
        Err(e) => (0, None, friendly_net_err(&e.to_string())),
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
        Err(e) => (0, None, friendly_net_err(&e.to_string())),
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
