//! Configuration merge logic
//!
//! Implements the 4-layer merge with:
//! - Objects: deep-merge by key
//! - Arrays: REPLACE (last wins)
//! - Scalars: override (last wins)

use serde_json::Value;

/// Deep merge two JSON values.
///
/// Merge semantics:
/// - Objects: deep-merge by key (recursive)
/// - Arrays: REPLACE (second wins entirely)
/// - Scalars: override (second wins)
/// - Null: override (null can override any value)
pub fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        // Both objects: deep merge
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                let merged = if let Some(base_value) = base_map.remove(&key) {
                    deep_merge(base_value, overlay_value)
                } else {
                    overlay_value
                };
                base_map.insert(key, merged);
            }
            Value::Object(base_map)
        }

        // Arrays: REPLACE (no concatenation)
        (Value::Array(_), overlay @ Value::Array(_)) => overlay,

        // Scalars and any other case: overlay wins
        (_, overlay) => overlay,
    }
}

/// Merge multiple config layers in order (first is base, last has highest precedence)
pub fn merge_layers(layers: Vec<Value>) -> Value {
    layers.into_iter().fold(Value::Null, deep_merge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_scalar_override() {
        let base = json!({"timeout": 100});
        let overlay = json!({"timeout": 200});
        let result = deep_merge(base, overlay);
        assert_eq!(result["timeout"], 200);
    }

    #[test]
    fn test_object_deep_merge() {
        let base = json!({
            "cache": {
                "derived_data": "off",
                "spm": "off"
            }
        });
        let overlay = json!({
            "cache": {
                "derived_data": "on"
            }
        });
        let result = deep_merge(base, overlay);

        // derived_data should be overridden
        assert_eq!(result["cache"]["derived_data"], "on");
        // spm should be preserved
        assert_eq!(result["cache"]["spm"], "off");
    }

    #[test]
    fn test_array_replace() {
        let base = json!({
            "schemes": ["A", "B", "C"]
        });
        let overlay = json!({
            "schemes": ["X", "Y"]
        });
        let result = deep_merge(base, overlay);

        // Array should be completely replaced
        let schemes = result["schemes"].as_array().unwrap();
        assert_eq!(schemes.len(), 2);
        assert_eq!(schemes[0], "X");
        assert_eq!(schemes[1], "Y");
    }

    #[test]
    fn test_add_new_key() {
        let base = json!({"a": 1});
        let overlay = json!({"b": 2});
        let result = deep_merge(base, overlay);

        assert_eq!(result["a"], 1);
        assert_eq!(result["b"], 2);
    }

    #[test]
    fn test_null_override() {
        let base = json!({"value": 100});
        let overlay = json!({"value": null});
        let result = deep_merge(base, overlay);

        assert!(result["value"].is_null());
    }

    #[test]
    fn test_merge_layers() {
        let builtin = json!({
            "timeout": 100,
            "cache": {"mode": "off"}
        });
        let host = json!({
            "timeout": 200
        });
        let repo = json!({
            "cache": {"mode": "on"}
        });
        let cli = json!({
            "timeout": 50
        });

        let result = merge_layers(vec![builtin, host, repo, cli]);

        // CLI wins for timeout
        assert_eq!(result["timeout"], 50);
        // Repo wins for cache.mode
        assert_eq!(result["cache"]["mode"], "on");
    }

    #[test]
    fn test_nested_deep_merge() {
        let base = json!({
            "level1": {
                "level2": {
                    "a": 1,
                    "b": 2
                }
            }
        });
        let overlay = json!({
            "level1": {
                "level2": {
                    "b": 3,
                    "c": 4
                }
            }
        });
        let result = deep_merge(base, overlay);

        assert_eq!(result["level1"]["level2"]["a"], 1);
        assert_eq!(result["level1"]["level2"]["b"], 3);
        assert_eq!(result["level1"]["level2"]["c"], 4);
    }
}
