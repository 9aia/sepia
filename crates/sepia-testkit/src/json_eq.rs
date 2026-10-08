//! Numeric-tolerant JSON equality — JS has one `number` type, Rust splits
//! `i64`/`u64`/`f64`, so `1` (TS) may legitimately land as `1.0` (Rust).
//! Compare values semantically, not by representation.

use std::fmt::Write as _;

use serde_json::Value;

/// Deep JSON equality with numeric normalization (1 == 1.0, NaN != NaN
/// like JS semantics where NaN !== NaN — though NaN can't appear in JSON).
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| json_eq(v, w))
        }
        (x, y) if x.is_number() && y.is_number() => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// Assert semantic JSON equality with a readable diff on failure.
pub fn assert_json_eq(actual: &Value, expected: &Value, context: &str) {
    if json_eq(actual, expected) {
        return;
    }
    let mut diff = String::new();
    collect_diff(actual, expected, "$", &mut diff, 50);
    panic!(
        "JSON mismatch in {context}:\n{diff}\n\nactual:   {}\nexpected: {}",
        truncate(&serde_json::to_string_pretty(actual).unwrap_or_default()),
        truncate(&serde_json::to_string_pretty(expected).unwrap_or_default()),
    );
}

fn truncate(s: &str) -> String {
    const MAX: usize = 4000;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…(truncated)", &s[..MAX])
    }
}

fn collect_diff(actual: &Value, expected: &Value, path: &str, out: &mut String, budget: usize) {
    if out.lines().count() >= budget {
        return;
    }
    if json_eq(actual, expected) {
        return;
    }
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            for (k, v) in e {
                let child = format!("{path}.{k}");
                match a.get(k) {
                    Some(av) => collect_diff(av, v, &child, out, budget),
                    None => {
                        let _ = writeln!(out, "- missing key {child}");
                    }
                }
            }
            for k in a.keys() {
                if !e.contains_key(k) {
                    let _ = writeln!(out, "+ unexpected key {path}.{k}");
                }
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            for (i, (av, ev)) in a.iter().zip(e.iter()).enumerate() {
                collect_diff(av, ev, &format!("{path}[{i}]"), out, budget);
            }
            if a.len() != e.len() {
                let _ = writeln!(
                    out,
                    "  length differs at {path}: actual {} vs expected {}",
                    a.len(),
                    e.len()
                );
            }
        }
        _ => {
            let _ = writeln!(
                out,
                "  value differs at {path}: actual {actual} vs expected {expected}"
            );
        }
    }
}
