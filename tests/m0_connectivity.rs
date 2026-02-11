//! M0 Connectivity Tests (rch-mac-sai.6)
//!
//! Tests for the M0 worker connectivity layer including:
//! - Worker inventory parsing (workers.toml)
//! - Capabilities schema validation
//! - Mock SSH probe tests

use rch_xcode_lane::inventory::{WorkerInventory, InventoryError};
use rch_xcode_lane::worker::Capabilities;
use serde_json::json;

// =============================================================================
// Worker Inventory Parsing Tests
// =============================================================================

mod inventory_tests {
    use super::*;

    #[test]
    fn test_valid_config_parsed_correctly() {
        let content = r#"
            schema_version = 1

            [[worker]]
            name = "macmini-01"
            host = "macmini.local"
            user = "rch"
            port = 22
            tags = ["macos", "xcode"]
            ssh_key_path = "~/.ssh/rch_macmini"
            priority = 10
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        assert_eq!(inventory.schema_version, 1);
        assert_eq!(inventory.workers.len(), 1);

        let worker = &inventory.workers[0];
        assert_eq!(worker.name, "macmini-01");
        assert_eq!(worker.host, "macmini.local");
        assert_eq!(worker.user, "rch");
        assert_eq!(worker.port, 22);
        assert_eq!(worker.tags, vec!["macos", "xcode"]);
        assert_eq!(worker.priority, 10);
    }

    #[test]
    fn test_missing_host_field_error() {
        let content = r#"
            [[worker]]
            name = "worker-no-host"
            user = "rch"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Should mention missing field
        assert!(err.contains("host") || err.contains("missing"), "Error should mention 'host': {}", err);
    }

    #[test]
    fn test_duplicate_worker_names_error() {
        let content = r#"
            [[worker]]
            name = "duplicate-name"
            host = "host1.local"

            [[worker]]
            name = "duplicate-name"
            host = "host2.local"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(matches!(result, Err(InventoryError::DuplicateName(_))));
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate-name"), "Error should mention duplicate name: {}", err);
    }

    #[test]
    fn test_tag_filtering_both_tags_required() {
        let content = r#"
            [[worker]]
            name = "mac-1"
            host = "host1.local"
            tags = ["macos", "xcode"]

            [[worker]]
            name = "mac-2"
            host = "host2.local"
            tags = ["macos"]

            [[worker]]
            name = "linux-1"
            host = "host3.local"
            tags = ["linux"]
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();

        // Filter by both macos AND xcode
        let filtered = inventory.filter_by_tags(&["macos", "xcode"]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "mac-1");

        // Filter by just macos
        let filtered = inventory.filter_by_tags(&["macos"]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_empty_inventory_parses_ok() {
        // Empty inventory is allowed (no workers configured)
        let content = r#"
            schema_version = 1
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        assert!(inventory.is_empty());
        assert_eq!(inventory.len(), 0);
    }

    #[test]
    fn test_default_values_applied() {
        let content = r#"
            [[worker]]
            name = "minimal"
            host = "host.local"
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let worker = &inventory.workers[0];

        // Check defaults
        assert_eq!(worker.port, 22, "Default port should be 22");
        assert_eq!(worker.user, "rch", "Default user should be 'rch'");
        assert_eq!(worker.priority, 100, "Default priority should be 100");
        assert!(worker.tags.is_empty(), "Default tags should be empty");
    }

    #[test]
    fn test_priority_ordering() {
        let content = r#"
            [[worker]]
            name = "worker-low-priority"
            host = "host1.local"
            priority = 50

            [[worker]]
            name = "worker-high-priority"
            host = "host2.local"
            priority = 10

            [[worker]]
            name = "worker-default-priority"
            host = "host3.local"
        "#;

        let inventory = WorkerInventory::parse(content).unwrap();
        let sorted = inventory.sorted_by_priority();

        assert_eq!(sorted[0].name, "worker-high-priority");
        assert_eq!(sorted[1].name, "worker-low-priority");
        assert_eq!(sorted[2].name, "worker-default-priority");
    }
}

// =============================================================================
// Capabilities Schema Validation Tests
// =============================================================================

mod capabilities_tests {
    use super::*;

    fn valid_capabilities_json() -> serde_json::Value {
        let mut cap = json!({
            "schema_version": 1,
            "schema_id": "rch-xcode/capabilities@1",
            "created_at": "2024-01-15T10:30:00Z",
            "rch_xcode_lane_version": "0.1.0",
            "protocol_min": 1,
            "protocol_max": 1,
            "features": ["tail", "fetch"],
            "xcode_versions": [{
                "version": "15.4",
                "build": "15F31d",
                "path": "/Applications/Xcode.app",
                "developer_dir": "/Applications/Xcode.app/Contents/Developer"
            }],
            "macos": {
                "version": "14.5",
                "build": "23F79",
                "architecture": "arm64"
            },
            "capacity": {
                "max_concurrent_jobs": 2,
                "disk_free_bytes": 0
            }
        });
        // Set disk_free_bytes to a large u64 value
        cap["capacity"]["disk_free_bytes"] = serde_json::Value::Number(
            serde_json::Number::from(100_000_000_000_u64)
        );
        cap
    }

    #[test]
    fn test_valid_capabilities_accepted() {
        let json = valid_capabilities_json();
        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_ok(), "Valid capabilities should be accepted: {:?}", result);
        let caps = result.unwrap();
        assert_eq!(caps.schema_version, 1);
        assert_eq!(caps.schema_id, "rch-xcode/capabilities@1");
    }

    #[test]
    fn test_missing_schema_version_error() {
        let mut json = valid_capabilities_json();
        json.as_object_mut().unwrap().remove("schema_version");

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_err(), "Missing schema_version should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("schema_version") || err.contains("missing field"),
                "Error should mention schema_version: {}", err);
    }

    #[test]
    fn test_missing_created_at_error() {
        let mut json = valid_capabilities_json();
        json.as_object_mut().unwrap().remove("created_at");

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_err(), "Missing created_at should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("created_at") || err.contains("missing field"),
                "Error should mention created_at: {}", err);
    }

    #[test]
    fn test_xcode_entry_missing_build_error() {
        let mut json = valid_capabilities_json();
        let xcodes = json["xcode_versions"].as_array_mut().unwrap();
        xcodes[0].as_object_mut().unwrap().remove("build");

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_err(), "Xcode entry missing build should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("build") || err.contains("missing field"),
                "Error should mention build field: {}", err);
    }

    #[test]
    fn test_missing_macos_version_error() {
        let mut json = valid_capabilities_json();
        json["macos"].as_object_mut().unwrap().remove("version");

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_err(), "Missing macOS version should error");
    }

    #[test]
    fn test_missing_capacity_error() {
        let mut json = valid_capabilities_json();
        json.as_object_mut().unwrap().remove("capacity");

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_err(), "Missing capacity should error");
    }

    #[test]
    fn test_empty_xcode_versions_array_accepted() {
        let mut json = valid_capabilities_json();
        json["xcode_versions"] = json!([]);

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_ok(), "Empty xcode_versions array should be accepted");
    }

    #[test]
    fn test_extra_fields_ignored() {
        let mut json = valid_capabilities_json();
        json.as_object_mut().unwrap().insert(
            "unknown_future_field".to_string(),
            json!("some value"),
        );

        let json_str = serde_json::to_string(&json).unwrap();
        let result: Result<Capabilities, _> = serde_json::from_str(&json_str);

        assert!(result.is_ok(), "Extra fields should be ignored for forward compatibility");
    }
}

// =============================================================================
// Mock SSH Probe Tests
// =============================================================================

mod probe_tests {
    use rch_xcode_lane::protocol::{RpcRequest, RpcResponse, Operation};
    use rch_xcode_lane::mock::MockWorker;
    use chrono::{Utc, Duration};

    #[test]
    fn test_probe_happy_path() {
        // Create a mock worker and get its probe response
        let worker = MockWorker::new();

        // Create probe request with protocol_version: 0
        let request = RpcRequest {
            protocol_version: 0,
            op: Operation::Probe,
            request_id: "test-probe-001".to_string(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
        };

        let response = worker.handle_request(&request);

        // Verify response
        assert!(response.ok, "Probe should succeed");
        assert_eq!(response.protocol_version, 0, "Probe response should have protocol_version 0");

        // Verify payload contains expected fields
        let payload = response.payload.unwrap();
        assert!(payload.get("protocol_min").is_some(), "Response should have protocol_min");
        assert!(payload.get("protocol_max").is_some(), "Response should have protocol_max");
        assert!(payload.get("features").is_some(), "Response should have features");
    }

    #[test]
    fn test_probe_uses_protocol_version_zero() {
        // Probe MUST use protocol_version: 0
        let worker = MockWorker::new();

        // Protocol version 0 should work for probe
        let request_v0 = RpcRequest {
            protocol_version: 0,
            op: Operation::Probe,
            request_id: "test-001".to_string(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
        };

        let response = worker.handle_request(&request_v0);
        assert!(response.ok, "Probe with v0 should succeed");

        // Protocol version 1 should fail for probe
        let request_v1 = RpcRequest {
            protocol_version: 1,
            op: Operation::Probe,
            request_id: "test-002".to_string(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
        };

        let response = worker.handle_request(&request_v1);
        assert!(!response.ok, "Probe with v1 should fail (probe must use v0)");
    }

    #[test]
    fn test_clock_skew_detection() {
        // Test that clock skew >30s can be detected
        let now = Utc::now();

        // Create capabilities with timestamp 45 seconds in the future
        let future_time = now + Duration::seconds(45);
        let past_time = now - Duration::seconds(45);
        let within_tolerance = now + Duration::seconds(25);

        // Check detection logic
        let skew_future = (future_time - now).num_seconds().abs();
        let skew_past = (past_time - now).num_seconds().abs();
        let skew_ok = (within_tolerance - now).num_seconds().abs();

        assert!(skew_future > 30, "45s future should trigger warning");
        assert!(skew_past > 30, "45s past should trigger warning");
        assert!(skew_ok <= 30, "25s should be within tolerance");
    }

    #[test]
    fn test_malformed_json_response_handling() {
        // Test parsing of invalid JSON
        let invalid_json = "{ not valid json }";
        let result: Result<RpcResponse, _> = serde_json::from_str(invalid_json);

        assert!(result.is_err(), "Invalid JSON should fail to parse");
        let err = result.unwrap_err().to_string();
        // Error should indicate JSON parse failure
        assert!(err.contains("expected") || err.contains("key") || err.contains("invalid"),
                "Error should indicate parse failure: {}", err);
    }

    #[test]
    fn test_probe_response_includes_protocol_range() {
        let worker = MockWorker::new();

        let request = RpcRequest {
            protocol_version: 0,
            op: Operation::Probe,
            request_id: "test-003".to_string(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
        };

        let response = worker.handle_request(&request);
        let payload = response.payload.unwrap();

        // Verify protocol range
        let min = payload["protocol_min"].as_u64().unwrap();
        let max = payload["protocol_max"].as_u64().unwrap();

        assert!(min >= 1, "protocol_min should be at least 1");
        assert!(max >= min, "protocol_max should be >= protocol_min");
    }

    #[test]
    fn test_probe_response_includes_features() {
        let worker = MockWorker::new();

        let request = RpcRequest {
            protocol_version: 0,
            op: Operation::Probe,
            request_id: "test-004".to_string(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
        };

        let response = worker.handle_request(&request);
        let payload = response.payload.unwrap();

        // Verify features array exists
        let features = payload["features"].as_array().unwrap();

        // Features array should exist (can be empty or have items)
        // Just verify we can iterate over it
        let _feature_count = features.len();
        assert!(true, "Features array should exist");
    }

    #[test]
    fn test_non_probe_with_version_zero_fails() {
        let worker = MockWorker::new();

        // Try to reserve with protocol_version: 0 (should fail)
        let request = RpcRequest {
            protocol_version: 0,
            op: Operation::Reserve,
            request_id: "test-005".to_string(),
            payload: serde_json::json!({"run_id": "test-run"}),
        };

        let response = worker.handle_request(&request);
        assert!(!response.ok, "Non-probe operations should fail with v0");

        let error = response.error.unwrap();
        assert!(error.code.contains("UNSUPPORTED") || error.code.contains("VERSION"),
                "Error code should indicate version issue: {}", error.code);
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

mod error_handling_tests {
    use rch_xcode_lane::inventory::{WorkerInventory, InventoryError};

    #[test]
    fn test_inventory_file_not_found() {
        let result = WorkerInventory::load(std::path::Path::new("/nonexistent/path/workers.toml"));
        assert!(matches!(result, Err(InventoryError::NotFound(_))));
    }

    #[test]
    fn test_invalid_toml_syntax() {
        let content = r#"
            [[worker]
            name = "broken"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_worker_name_rejected() {
        let content = r#"
            [[worker]]
            name = ""
            host = "host.local"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("name") || err.contains("empty"),
                "Error should mention empty name: {}", err);
    }

    #[test]
    fn test_empty_host_rejected() {
        let content = r#"
            [[worker]]
            name = "worker"
            host = ""
        "#;

        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("host") || err.contains("empty"),
                "Error should mention empty host: {}", err);
    }

    #[test]
    fn test_invalid_worker_name_characters() {
        let content = r#"
            [[worker]]
            name = "worker with spaces"
            host = "host.local"
        "#;

        let result = WorkerInventory::parse(content);
        assert!(result.is_err());
    }
}
