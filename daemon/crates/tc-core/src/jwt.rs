//! JWT parsing (no signature verification — local display/routing only).
//! Port of `src-tauri/src/jwt.rs`: supports `Cloud-IDE-JWT <token>` / `Bearer <token>`
//! prefixes; user_id taken from payload `data.id` → `auth_id` → `sub`.

use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JwtInfo {
    pub user_id: Option<String>,
    pub exp_hours: Option<f64>,
    pub exp_timestamp: Option<i64>,
}

pub fn parse(jwt_full: &str) -> JwtInfo {
    let trimmed = jwt_full.trim();
    let token = trimmed
        .strip_prefix("Cloud-IDE-JWT ")
        .or_else(|| trimmed.strip_prefix("Bearer "))
        .unwrap_or(trimmed)
        .trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return JwtInfo::default();
    }
    let payload_b64 = parts[1].trim_end_matches('=');
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64) else {
        return JwtInfo::default();
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return JwtInfo::default();
    };
    let user_id = payload
        .get("data")
        .and_then(|d| d.get("id"))
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        .or_else(|| payload.get("auth_id").and_then(|v| v.as_str()).map(String::from))
        .or_else(|| payload.get("sub").and_then(|v| v.as_str()).map(String::from));

    let exp_timestamp = payload.get("exp").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    });

    let exp_hours = exp_timestamp.map(|exp| {
        let now = chrono::Utc::now().timestamp();
        (exp - now) as f64 / 3600.0
    });

    JwtInfo { user_id, exp_hours, exp_timestamp }
}

/// Derive status from exp remaining hours: >24 ok / <=24 && >0 warn / <=0 expired.
pub fn status_of(exp_hours: Option<f64>) -> &'static str {
    match exp_hours {
        Some(h) if h > 24.0 => "ok",
        Some(h) if h > 0.0 => "warn",
        Some(_) => "expired",
        None => "unknown",
    }
}

/// WB OAuth JWT claims decode (no signature check): full payload object.
pub fn claims(token: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let seg = parts[1].trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJkYXRhIjp7ImlkIjoiMTIzNDU2Nzg5MCJ9LCJleHAiOjk5OTk5OTk5OTl9.sig";

    #[test]
    fn parse_extracts_user_id() {
        let info = parse(TOKEN);
        assert_eq!(info.user_id.as_deref(), Some("1234567890"));
        // exp far in the future
        assert!(info.exp_hours.unwrap_or(-1.0) > 24.0);
        assert_eq!(status_of(info.exp_hours), "ok");
    }

    #[test]
    fn parse_accepts_prefixes() {
        assert_eq!(parse(&format!("Cloud-IDE-JWT {TOKEN}")).user_id, parse(TOKEN).user_id);
        assert_eq!(parse(&format!("Bearer {TOKEN}")).user_id, parse(TOKEN).user_id);
    }

    #[test]
    fn parse_garbage_is_default() {
        assert_eq!(parse("not-a-jwt").user_id, None);
        assert_eq!(parse("").user_id, None);
        assert_eq!(status_of(None), "unknown");
    }
}
