//! Small `serde_json` helpers shared across crates.

use serde_json::Value;

/// The string value at `key`, or `""` if the key is missing or not a string.
pub fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_or_defaults_empty() {
        let v = json!({"a": "x", "b": 3});
        assert_eq!(str_field(&v, "a"), "x");
        assert_eq!(str_field(&v, "b"), ""); // not a string
        assert_eq!(str_field(&v, "missing"), "");
    }
}
