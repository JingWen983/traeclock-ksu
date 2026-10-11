//! Tolerant JSON envelope digging — port of `src-tauri/src/fs_utils.rs`
//! (`dig`/`path`, ENVELOPE_KEYS, DIG_MAX_DEPTH) and the trae_checkin unwrap
//! helpers (`find_payload_field`, `parse_claim_reward`).
//!
//! Semantics red line (locked by upstream tests): `dig` keys are *candidate
//! synonym keys* searched across the whole tree through envelope wrappers;
//! `path` walks literal segments only.

use serde_json::Value;

pub const ENVELOPE_KEYS: [&str; 7] = ["data", "result", "resp", "response", "info", "auth", "account"];
pub const DIG_MAX_DEPTH: usize = 8;

/// Find any of `keys` in `v`, first hit wins (whole-tree search through envelopes).
pub fn dig<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| dig_key(v, k, 0))
}

/// Strict path walk: literal segments only, no envelope hopping.
pub fn path<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for k in keys {
        cur = cur.get(*k)?;
    }
    Some(cur)
}

fn dig_key<'a>(v: &'a Value, key: &str, depth: usize) -> Option<&'a Value> {
    if depth > DIG_MAX_DEPTH {
        return None;
    }
    match v {
        Value::Object(map) => {
            if let Some(hit) = map.get(key) {
                return Some(hit);
            }
            ENVELOPE_KEYS
                .iter()
                .find_map(|wk| map.get(*wk).and_then(|child| dig_key(child, key, depth + 1)))
        }
        Value::Array(arr) => arr.iter().find_map(|item| dig_key(item, key, depth + 1)),
        _ => None,
    }
}

// ── trae_checkin unwrap helpers ─────────────────────────────────────────────

/// Payload wrapper keys (outer → inner), from `tasks/trae_checkin.rs`.
pub const PAYLOAD_WRAPPER_KEYS: [&str; 5] = ["data", "result", "resp", "response", "info"];
pub const UNWRAP_MAX_DEPTH: usize = 8;

fn unwrap_walk<'a>(obj: &'a Value, depth: usize, scopes: &mut Vec<&'a Value>) {
    let Some(map) = obj.as_object() else { return };
    if depth > UNWRAP_MAX_DEPTH {
        return;
    }
    scopes.push(obj);
    for k in PAYLOAD_WRAPPER_KEYS {
        if let Some(child) = map.get(k).filter(|v| v.is_object()) {
            unwrap_walk(child, depth + 1, scopes);
        }
    }
}

/// Walk envelope keys, returning [outermost, …, innermost business object].
pub fn unwrap_scopes(data: &Value) -> Vec<&Value> {
    let mut scopes = Vec::new();
    unwrap_walk(data, 0, &mut scopes);
    scopes
}

/// Find `key` per scope (outer wins, top-level semantics preserved).
pub fn find_payload_field<'a>(data: &'a Value, key: &str) -> Option<&'a Value> {
    unwrap_scopes(data).into_iter().find_map(|s| s.get(key))
}

/// Tolerant int normalization (port of python `_as_int` / Rust `as_int_tolerant`):
/// int / integral float / numeric string → i64; bool / fractional / other → None.
/// Negative ints are preserved (caller decides semantics).
pub fn as_int_tolerant(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else {
                let f = n.as_f64()?;
                (f.fract() == 0.0).then(|| f as i64)
            }
        }
        Value::String(s) => {
            let t = s.trim();
            if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
                t.parse::<i64>().ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Candidate keys for "the reward granted by this claim" (priority order),
/// data layer preferred over top level.
pub const CLAIM_REWARD_KEYS: [&str; 10] = [
    "reward", "reward_credits", "claim_credits", "checkin_credits", "delta", "increase", "obtain",
    "gained", "credits", "amount",
];

/// Extract the claim reward from a claim response (pure function, port of
/// `parse_claim_reward`): inner business object first, positive ints only,
/// zero placeholders are skipped.
pub fn parse_claim_reward(data: Option<&Value>) -> Option<i64> {
    let d = data?;
    let mut scopes = unwrap_scopes(d);
    scopes.reverse(); // inner first
    for scope in scopes {
        for key in CLAIM_REWARD_KEYS {
            if let Some(n) = scope.get(key).and_then(as_int_tolerant) {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dig_searches_candidate_keys_through_envelopes() {
        let v = json!({
            "ResponseMetadata": {"Error": {"Code": "20101", "Message": "refresh token is invalid"}}
        });
        // path semantics: literal walk → finds the Code string
        assert_eq!(
            path(&v, &["ResponseMetadata", "Error", "Code"]).and_then(|x| x.as_str()),
            Some("20101")
        );
        // dig semantics: keys are candidates — "ResponseMetadata" hit first
        // returns the CONTAINER object itself (source-locked distinction)
        let d = dig(&v, &["ResponseMetadata", "Error", "Code"]).unwrap();
        assert!(d.is_object(), "dig returns the first candidate hit (a container)");
        // ResponseMetadata/Error are NOT envelope keys → dig cannot reach the
        // nested Code/Message leaves (that's what path is for, source-locked)
        assert_eq!(dig(&v, &["Code"]), None);
        assert_eq!(dig(&v, &["Message"]), None);
        // real envelope keys do get searched through
        let v2 = json!({"data": {"Code": "20101"}});
        assert_eq!(dig(&v2, &["Code"]).and_then(|x| x.as_str()), Some("20101"));
    }

    #[test]
    fn dig_handles_array_wrapping() {
        // top-level array: each element searched
        let v = json!([{"accessToken": "tok-1"}]);
        assert_eq!(dig(&v, &["accessToken"]).and_then(|x| x.as_str()), Some("tok-1"));
        // named envelope key containing an array
        let v2 = json!({"data": [{"refreshToken": "rt-1"}]});
        assert_eq!(dig(&v2, &["refreshToken"]).and_then(|x| x.as_str()), Some("rt-1"));
        // non-envelope key wrapping an array is NOT searched (source semantics)
        let v3 = json!({"items": [{"accessToken": "tok-2"}]});
        assert_eq!(dig(&v3, &["accessToken"]), None);
    }

    #[test]
    fn find_field_outer_wins() {
        let payload = json!({"code": 0, "data": {"result": {"credits": 99}}, "credits": 1});
        assert_eq!(find_payload_field(&payload, "credits"), Some(&json!(1)));
        let deep = json!({"data": {"result": {"credits": 42}}});
        assert_eq!(find_payload_field(&deep, "credits"), Some(&json!(42)));
        assert_eq!(find_payload_field(&json!({"other": 1}), "credits"), None);
        assert_eq!(find_payload_field(&json!(null), "credits"), None);
    }

    #[test]
    fn tolerant_int_normalization() {
        assert_eq!(as_int_tolerant(&json!(5)), Some(5));
        assert_eq!(as_int_tolerant(&json!(2.0)), Some(2));
        assert_eq!(as_int_tolerant(&json!("15")), Some(15));
        assert_eq!(as_int_tolerant(&json!(true)), None);
        assert_eq!(as_int_tolerant(&json!(-5)), Some(-5));
        assert_eq!(as_int_tolerant(&json!(1.5)), None);
        assert_eq!(as_int_tolerant(&json!("abc")), None);
        assert_eq!(as_int_tolerant(&json!(null)), None);
    }

    #[test]
    fn claim_reward_priority_and_zero_skip() {
        assert_eq!(parse_claim_reward(Some(&json!({"reward": 10}))), Some(10));
        // data layer preferred over top level
        assert_eq!(
            parse_claim_reward(Some(&json!({"data": {"delta": 5}, "credits": 99}))),
            Some(5)
        );
        // zero placeholder doesn't block real reward
        assert_eq!(
            parse_claim_reward(Some(&json!({"code": 0, "data": {"credits": 0, "reward": 12}}))),
            Some(12)
        );
        // multi-layer envelope
        assert_eq!(
            parse_claim_reward(Some(&json!({"code": 0, "data": {"data": {"reward": 9}}}))),
            Some(9)
        );
        // negative / fractional / bool rejected
        assert_eq!(parse_claim_reward(Some(&json!({"delta": -1}))), None);
        assert_eq!(parse_claim_reward(Some(&json!({"delta": 3.5}))), None);
        assert_eq!(parse_claim_reward(Some(&json!({"reward": true}))), None);
        assert_eq!(parse_claim_reward(Some(&json!({"amount": 0}))), None);
        assert_eq!(parse_claim_reward(None), None);
    }
}
