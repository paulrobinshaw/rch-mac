//! Conformance Test Suite (M7, bead b7s.5)
//!
//! Provides a single `rch xcode conformance` runner that executes all
//! conformance categories and produces structured JSON reports.
//!
//! Categories:
//! - JobSpec determinism: identical inputs → identical job_key
//! - Protocol round-trip: RPC serialization/deserialization
//! - Schema compliance: artifact schema validation
//! - Cross-milestone integration: artifact graph consistency

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::destination::Provisioning;
use crate::job::{JobKeyDestination, JobKeyInputs, JobKeyToolchain};

/// Schema version for conformance reports
pub const CONFORMANCE_SCHEMA_VERSION: u32 = 1;

/// Schema identifier for conformance reports
pub const CONFORMANCE_SCHEMA_ID: &str = "rch-xcode/conformance_report@1";

/// Overall conformance test report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceReport {
    /// Schema version
    pub schema_version: u32,
    /// Schema identifier
    pub schema_id: String,
    /// When the report was generated
    pub created_at: DateTime<Utc>,
    /// Overall pass/fail status
    pub passed: bool,
    /// Number of categories tested
    pub category_count: usize,
    /// Total number of tests
    pub test_count: usize,
    /// Number of passed tests
    pub passed_count: usize,
    /// Number of failed tests
    pub failed_count: usize,
    /// Total duration in milliseconds
    pub duration_ms: u64,
    /// Per-category results
    pub categories: Vec<CategoryReport>,
}

impl ConformanceReport {
    /// Create a new empty report
    pub fn new() -> Self {
        Self {
            schema_version: CONFORMANCE_SCHEMA_VERSION,
            schema_id: CONFORMANCE_SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            passed: true,
            category_count: 0,
            test_count: 0,
            passed_count: 0,
            failed_count: 0,
            duration_ms: 0,
            categories: Vec::new(),
        }
    }

    /// Add a category report
    pub fn add_category(&mut self, cat: CategoryReport) {
        self.category_count += 1;
        self.test_count += cat.test_count;
        self.passed_count += cat.passed_count;
        self.failed_count += cat.failed_count;
        self.duration_ms += cat.duration_ms;
        if !cat.passed {
            self.passed = false;
        }
        self.categories.push(cat);
    }

    /// Finalize the report
    pub fn finalize(&mut self) {
        self.passed = self.failed_count == 0;
    }

    /// Convert to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Get exit code (0 = pass, 1 = fail)
    pub fn exit_code(&self) -> i32 {
        if self.passed {
            0
        } else {
            1
        }
    }

    /// Print human-readable summary
    pub fn print_summary(&self) {
        println!("\n=== Conformance Test Summary ===");
        println!(
            "Status: {}",
            if self.passed { "PASS" } else { "FAIL" }
        );
        println!(
            "Tests: {} total, {} passed, {} failed",
            self.test_count, self.passed_count, self.failed_count
        );
        println!("Duration: {:.2}s", self.duration_ms as f64 / 1000.0);
        println!();

        for cat in &self.categories {
            let status = if cat.passed { "✓" } else { "✗" };
            println!(
                "  {} {} ({}/{} passed)",
                status, cat.category, cat.passed_count, cat.test_count
            );

            for result in &cat.results {
                if !result.pass {
                    println!("      ✗ {}", result.test_name);
                    if let Some(ref err) = result.error {
                        println!("        {}", err);
                    }
                }
            }
        }
    }
}

impl Default for ConformanceReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-category test results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryReport {
    /// Category name
    pub category: String,
    /// Overall pass/fail for this category
    pub passed: bool,
    /// Number of tests in category
    pub test_count: usize,
    /// Number passed
    pub passed_count: usize,
    /// Number failed
    pub failed_count: usize,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Individual test results
    pub results: Vec<TestResult>,
}

impl CategoryReport {
    /// Create a new category report
    pub fn new(category: String) -> Self {
        Self {
            category,
            passed: true,
            test_count: 0,
            passed_count: 0,
            failed_count: 0,
            duration_ms: 0,
            results: Vec::new(),
        }
    }

    /// Add a test result
    pub fn add_result(&mut self, result: TestResult) {
        self.test_count += 1;
        self.duration_ms += result.duration_ms;
        if result.pass {
            self.passed_count += 1;
        } else {
            self.failed_count += 1;
            self.passed = false;
        }
        self.results.push(result);
    }
}

/// Individual test result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// Test name
    pub test_name: String,
    /// Whether the test passed
    pub pass: bool,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TestResult {
    /// Create a passing result
    pub fn pass(test_name: &str, duration: Duration) -> Self {
        Self {
            test_name: test_name.to_string(),
            pass: true,
            duration_ms: duration.as_millis() as u64,
            error: None,
        }
    }

    /// Create a failing result
    pub fn fail(test_name: &str, duration: Duration, message: String) -> Self {
        Self {
            test_name: test_name.to_string(),
            pass: false,
            duration_ms: duration.as_millis() as u64,
            error: Some(message),
        }
    }

    /// Create an error result (test could not run)
    pub fn error(test_name: &str, message: String) -> Self {
        Self {
            test_name: test_name.to_string(),
            pass: false,
            duration_ms: 0,
            error: Some(message),
        }
    }
}

/// Conformance test runner
pub struct ConformanceRunner {
    #[allow(dead_code)]
    verbose: bool,
}

impl ConformanceRunner {
    /// Create a new conformance runner
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    /// Run all conformance tests and return the report
    pub fn run_all(&self) -> ConformanceReport {
        let mut report = ConformanceReport::new();

        report.add_category(self.run_jobspec_determinism());
        report.add_category(self.run_protocol_roundtrip());
        report.add_category(self.run_schema_compliance());
        report.add_category(self.run_cross_milestone());

        report.finalize();
        report
    }

    /// Run only specified categories
    pub fn run_categories(&self, categories: &[&str]) -> ConformanceReport {
        let mut report = ConformanceReport::new();

        for cat in categories {
            match *cat {
                "jobspec" => report.add_category(self.run_jobspec_determinism()),
                "protocol" => report.add_category(self.run_protocol_roundtrip()),
                "schema" => report.add_category(self.run_schema_compliance()),
                "integration" => report.add_category(self.run_cross_milestone()),
                _ => {
                    let mut cat_report = CategoryReport::new(cat.to_string());
                    cat_report.add_result(TestResult::error(
                        "unknown_category",
                        format!("Unknown category: {}", cat),
                    ));
                    report.add_category(cat_report);
                }
            }
        }

        report.finalize();
        report
    }

    /// Run JobSpec determinism tests
    fn run_jobspec_determinism(&self) -> CategoryReport {
        let mut cat = CategoryReport::new("jobspec_determinism".to_string());

        cat.add_result(self.test_job_key_identical_inputs());
        cat.add_result(self.test_job_key_stability());
        cat.add_result(self.test_job_key_source_sensitivity());
        cat.add_result(self.test_job_key_argv_sensitivity());
        cat.add_result(self.test_job_key_toolchain_sensitivity());
        cat.add_result(self.test_job_key_destination_sensitivity());
        cat.add_result(self.test_jcs_canonicalization());

        cat
    }

    fn test_job_key_identical_inputs(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "identical_inputs_produce_identical_key";

        let inputs = sample_job_key_inputs();
        let inputs_clone = inputs.clone();

        match (inputs.compute_job_key(), inputs_clone.compute_job_key()) {
            (Ok(key1), Ok(key2)) if key1 == key2 => TestResult::pass(test_name, start.elapsed()),
            (Ok(key1), Ok(key2)) => TestResult::fail(
                test_name,
                start.elapsed(),
                format!("Keys differ: {} != {}", key1, key2),
            ),
            (Err(e), _) | (_, Err(e)) => {
                TestResult::error(test_name, format!("Computation failed: {}", e))
            }
        }
    }

    fn test_job_key_stability(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "key_stable_across_computations";

        let inputs = sample_job_key_inputs();

        match (
            inputs.compute_job_key(),
            inputs.compute_job_key(),
            inputs.compute_job_key(),
        ) {
            (Ok(k1), Ok(k2), Ok(k3)) if k1 == k2 && k2 == k3 => {
                TestResult::pass(test_name, start.elapsed())
            }
            (Ok(k1), Ok(k2), Ok(k3)) => TestResult::fail(
                test_name,
                start.elapsed(),
                format!("Keys not stable: {}, {}, {}", k1, k2, k3),
            ),
            _ => TestResult::error(test_name, "Computation failed".to_string()),
        }
    }

    fn test_job_key_source_sensitivity(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "different_source_produces_different_key";

        let mut inputs1 = sample_job_key_inputs();
        let mut inputs2 = sample_job_key_inputs();
        inputs1.source_sha256 = "source_a".to_string();
        inputs2.source_sha256 = "source_b".to_string();

        match (inputs1.compute_job_key(), inputs2.compute_job_key()) {
            (Ok(key1), Ok(key2)) if key1 != key2 => TestResult::pass(test_name, start.elapsed()),
            (Ok(_), Ok(_)) => TestResult::fail(
                test_name,
                start.elapsed(),
                "Keys should differ but are equal".to_string(),
            ),
            _ => TestResult::error(test_name, "Computation failed".to_string()),
        }
    }

    fn test_job_key_argv_sensitivity(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "different_argv_produces_different_key";

        let mut inputs1 = sample_job_key_inputs();
        let mut inputs2 = sample_job_key_inputs();
        inputs1.sanitized_argv = vec!["build".to_string()];
        inputs2.sanitized_argv = vec!["test".to_string()];

        match (inputs1.compute_job_key(), inputs2.compute_job_key()) {
            (Ok(key1), Ok(key2)) if key1 != key2 => TestResult::pass(test_name, start.elapsed()),
            (Ok(_), Ok(_)) => TestResult::fail(
                test_name,
                start.elapsed(),
                "Keys should differ for different argv".to_string(),
            ),
            _ => TestResult::error(test_name, "Computation failed".to_string()),
        }
    }

    fn test_job_key_toolchain_sensitivity(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "different_toolchain_produces_different_key";

        let mut inputs1 = sample_job_key_inputs();
        let mut inputs2 = sample_job_key_inputs();
        inputs1.toolchain.xcode_build = "16C5032a".to_string();
        inputs2.toolchain.xcode_build = "17A5023a".to_string();

        match (inputs1.compute_job_key(), inputs2.compute_job_key()) {
            (Ok(key1), Ok(key2)) if key1 != key2 => TestResult::pass(test_name, start.elapsed()),
            (Ok(_), Ok(_)) => TestResult::fail(
                test_name,
                start.elapsed(),
                "Keys should differ for different toolchain".to_string(),
            ),
            _ => TestResult::error(test_name, "Computation failed".to_string()),
        }
    }

    fn test_job_key_destination_sensitivity(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "different_destination_produces_different_key";

        let mut inputs1 = sample_job_key_inputs();
        let mut inputs2 = sample_job_key_inputs();
        inputs1.destination.name = "iPhone 16".to_string();
        inputs2.destination.name = "iPhone 15".to_string();

        match (inputs1.compute_job_key(), inputs2.compute_job_key()) {
            (Ok(key1), Ok(key2)) if key1 != key2 => TestResult::pass(test_name, start.elapsed()),
            (Ok(_), Ok(_)) => TestResult::fail(
                test_name,
                start.elapsed(),
                "Keys should differ for different destination".to_string(),
            ),
            _ => TestResult::error(test_name, "Computation failed".to_string()),
        }
    }

    fn test_jcs_canonicalization(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "jcs_canonicalization";

        let inputs = sample_job_key_inputs();
        let json = match serde_json::to_string(&inputs) {
            Ok(j) => j,
            Err(e) => {
                return TestResult::error(test_name, format!("Serialization failed: {}", e))
            }
        };

        let parsed: JobKeyInputs = match serde_json::from_str(&json) {
            Ok(p) => p,
            Err(e) => {
                return TestResult::error(test_name, format!("Deserialization failed: {}", e))
            }
        };

        match (inputs.compute_job_key(), parsed.compute_job_key()) {
            (Ok(key1), Ok(key2)) if key1 == key2 => TestResult::pass(test_name, start.elapsed()),
            (Ok(key1), Ok(key2)) => TestResult::fail(
                test_name,
                start.elapsed(),
                format!("JCS not canonical: {} != {}", key1, key2),
            ),
            _ => TestResult::error(test_name, "Computation failed".to_string()),
        }
    }

    /// Run protocol round-trip tests
    fn run_protocol_roundtrip(&self) -> CategoryReport {
        let mut cat = CategoryReport::new("protocol_roundtrip".to_string());

        cat.add_result(self.test_rpc_request_roundtrip());
        cat.add_result(self.test_rpc_response_roundtrip());
        cat.add_result(self.test_operation_serialization());

        cat
    }

    fn test_rpc_request_roundtrip(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "rpc_request_roundtrip";

        use crate::{Operation, RpcRequest};

        let request = RpcRequest {
            protocol_version: 1,
            request_id: "test-123".to_string(),
            op: Operation::Probe,
            payload: serde_json::json!({"key": "value"}),
        };

        let json = match serde_json::to_string(&request) {
            Ok(j) => j,
            Err(e) => return TestResult::error(test_name, format!("Serialize failed: {}", e)),
        };

        let parsed: RpcRequest = match serde_json::from_str(&json) {
            Ok(p) => p,
            Err(e) => return TestResult::error(test_name, format!("Deserialize failed: {}", e)),
        };

        if parsed.protocol_version == request.protocol_version
            && parsed.request_id == request.request_id
            && parsed.op == request.op
        {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(test_name, start.elapsed(), "Fields don't match".to_string())
        }
    }

    fn test_rpc_response_roundtrip(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "rpc_response_roundtrip";

        use crate::RpcResponse;

        let response = RpcResponse {
            protocol_version: 1,
            request_id: "test-123".to_string(),
            ok: true,
            payload: Some(serde_json::json!({"result": "ok"})),
            error: None,
        };

        let json = match serde_json::to_string(&response) {
            Ok(j) => j,
            Err(e) => return TestResult::error(test_name, format!("Serialize failed: {}", e)),
        };

        let parsed: RpcResponse = match serde_json::from_str(&json) {
            Ok(p) => p,
            Err(e) => return TestResult::error(test_name, format!("Deserialize failed: {}", e)),
        };

        if parsed.request_id == response.request_id && parsed.ok == response.ok {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(test_name, start.elapsed(), "Fields don't match".to_string())
        }
    }

    fn test_operation_serialization(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "operation_serialization";

        use crate::Operation;

        let operations = vec![
            Operation::Probe,
            Operation::Submit,
            Operation::Status,
            Operation::Tail,
            Operation::Cancel,
        ];

        for op in operations {
            let json = match serde_json::to_string(&op) {
                Ok(j) => j,
                Err(e) => return TestResult::error(test_name, format!("Serialize failed: {}", e)),
            };

            let parsed: Operation = match serde_json::from_str(&json) {
                Ok(p) => p,
                Err(e) => {
                    return TestResult::error(test_name, format!("Deserialize failed: {}", e))
                }
            };

            if parsed != op {
                return TestResult::fail(
                    test_name,
                    start.elapsed(),
                    format!("Operation round-trip failed for {:?}", op),
                );
            }
        }

        TestResult::pass(test_name, start.elapsed())
    }

    /// Run schema compliance tests
    fn run_schema_compliance(&self) -> CategoryReport {
        let mut cat = CategoryReport::new("schema_compliance".to_string());

        cat.add_result(self.test_attestation_schema_fields());
        cat.add_result(self.test_run_index_schema());
        cat.add_result(self.test_job_index_schema());

        cat
    }

    fn test_attestation_schema_fields(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "attestation_has_required_fields";

        use crate::artifact::{Attestation, AttestationBackendIdentity, AttestationWorkerIdentity};

        let attestation = Attestation::from_components(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "source-hash".to_string(),
            AttestationWorkerIdentity {
                name: "worker-01".to_string(),
                fingerprint: "fp-123".to_string(),
            },
            "caps-hash".to_string(),
            AttestationBackendIdentity {
                name: "xcodebuild".to_string(),
                version: "15.0".to_string(),
            },
            "manifest-hash".to_string(),
        );

        let json = match serde_json::to_string(&attestation) {
            Ok(j) => j,
            Err(e) => return TestResult::error(test_name, format!("Serialize failed: {}", e)),
        };

        let value: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => return TestResult::error(test_name, format!("Parse failed: {}", e)),
        };

        let has_schema_version = value.get("schema_version").is_some();
        let has_job_id = value.get("job_id").is_some();
        let has_run_id = value.get("run_id").is_some();

        if has_schema_version && has_job_id && has_run_id {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(
                test_name,
                start.elapsed(),
                "Missing required fields".to_string(),
            )
        }
    }

    fn test_run_index_schema(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "run_index_has_required_fields";

        use crate::artifact::RunIndex;

        let index = RunIndex::new("run-123".to_string());

        let json = match index.to_json() {
            Ok(j) => j,
            Err(e) => return TestResult::error(test_name, format!("Serialize failed: {}", e)),
        };

        let value: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => return TestResult::error(test_name, format!("Parse failed: {}", e)),
        };

        let has_schema_version = value.get("schema_version").is_some();
        let has_run_id = value.get("run_id").is_some();

        if has_schema_version && has_run_id {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(
                test_name,
                start.elapsed(),
                "Missing required fields".to_string(),
            )
        }
    }

    fn test_job_index_schema(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "job_index_has_required_fields";

        use crate::artifact::JobIndex;

        let index = JobIndex::new(
            "run-123".to_string(),
            "job-456".to_string(),
            "key-789".to_string(),
            "build".to_string(),
        );

        let json = match index.to_json() {
            Ok(j) => j,
            Err(e) => return TestResult::error(test_name, format!("Serialize failed: {}", e)),
        };

        let value: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => return TestResult::error(test_name, format!("Parse failed: {}", e)),
        };

        let has_schema_version = value.get("schema_version").is_some();
        let has_job_id = value.get("job_id").is_some();

        if has_schema_version && has_job_id {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(
                test_name,
                start.elapsed(),
                "Missing required fields".to_string(),
            )
        }
    }

    /// Run cross-milestone integration tests
    fn run_cross_milestone(&self) -> CategoryReport {
        let mut cat = CategoryReport::new("cross_milestone_integration".to_string());

        cat.add_result(self.test_id_consistency());
        cat.add_result(self.test_schema_version_consistency());

        cat
    }

    fn test_id_consistency(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "artifact_ids_consistent";

        let run_id = "run-123".to_string();
        let job_id = "job-456".to_string();

        use crate::artifact::{
            Attestation, AttestationBackendIdentity, AttestationWorkerIdentity, JobIndex, RunIndex,
        };

        let attestation = Attestation::from_components(
            run_id.clone(),
            job_id.clone(),
            "key".to_string(),
            "source".to_string(),
            AttestationWorkerIdentity {
                name: "worker".to_string(),
                fingerprint: "fp".to_string(),
            },
            "caps".to_string(),
            AttestationBackendIdentity {
                name: "xcodebuild".to_string(),
                version: "15.0".to_string(),
            },
            "manifest".to_string(),
        );

        let run_index = RunIndex::new(run_id.clone());
        let job_index = JobIndex::new(run_id.clone(), job_id.clone(), "key".to_string(), "build".to_string());

        if attestation.run_id == run_id
            && attestation.job_id == job_id
            && run_index.run_id == run_id
            && job_index.run_id == run_id
            && job_index.job_id == job_id
        {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(
                test_name,
                start.elapsed(),
                "IDs not consistent across artifacts".to_string(),
            )
        }
    }

    fn test_schema_version_consistency(&self) -> TestResult {
        let start = Instant::now();
        let test_name = "schema_versions_are_v1";

        use crate::artifact::{
            ATTESTATION_SCHEMA_VERSION, JOB_INDEX_SCHEMA_VERSION, RUN_INDEX_SCHEMA_VERSION,
            SCHEMA_VERSION,
        };

        if SCHEMA_VERSION == 1
            && ATTESTATION_SCHEMA_VERSION == 1
            && RUN_INDEX_SCHEMA_VERSION == 1
            && JOB_INDEX_SCHEMA_VERSION == 1
        {
            TestResult::pass(test_name, start.elapsed())
        } else {
            TestResult::fail(
                test_name,
                start.elapsed(),
                "Schema versions should all be 1".to_string(),
            )
        }
    }
}

fn sample_job_key_inputs() -> JobKeyInputs {
    JobKeyInputs::new(
        "abc123def456789012345678901234567890123456789012345678901234".to_string(),
        vec![
            "build".to_string(),
            "-scheme".to_string(),
            "MyApp".to_string(),
        ],
        JobKeyToolchain {
            xcode_build: "16C5032a".to_string(),
            developer_dir: "/Applications/Xcode.app/Contents/Developer".to_string(),
            macos_version: "15.3".to_string(),
            macos_build: "24D60".to_string(),
            arch: "arm64".to_string(),
        },
        JobKeyDestination {
            platform: "iOS Simulator".to_string(),
            name: "iPhone 16".to_string(),
            os_version: "18.2".to_string(),
            provisioning: Provisioning::Existing,
            sim_runtime_identifier: Some(
                "com.apple.CoreSimulator.SimRuntime.iOS-18-2".to_string(),
            ),
            sim_runtime_build: Some("22C150".to_string()),
            device_type_identifier: Some(
                "com.apple.CoreSimulator.SimDeviceType.iPhone-16".to_string(),
            ),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_runs_all_categories() {
        let runner = ConformanceRunner::new(false);
        let report = runner.run_all();

        assert!(report.category_count > 0);
        assert!(report.test_count > 0);
    }

    #[test]
    fn test_runner_specific_category() {
        let runner = ConformanceRunner::new(false);
        let report = runner.run_categories(&["jobspec"]);

        assert_eq!(report.category_count, 1);
        assert!(report.test_count > 0);
    }

    #[test]
    fn test_report_json_output() {
        let runner = ConformanceRunner::new(false);
        let report = runner.run_all();

        let json = report.to_json();
        assert!(json.is_ok());

        let value: serde_json::Value = serde_json::from_str(&json.unwrap()).unwrap();
        assert!(value.get("schema_version").is_some());
        assert!(value.get("categories").is_some());
    }

    #[test]
    fn test_report_aggregation() {
        let mut report = ConformanceReport::new();

        let mut cat1 = CategoryReport::new("cat1".to_string());
        cat1.add_result(TestResult::pass("test1", Duration::from_millis(10)));
        cat1.add_result(TestResult::pass("test2", Duration::from_millis(20)));

        let mut cat2 = CategoryReport::new("cat2".to_string());
        cat2.add_result(TestResult::pass("test3", Duration::from_millis(15)));
        cat2.add_result(TestResult::fail(
            "test4",
            Duration::from_millis(25),
            "failed".to_string(),
        ));

        report.add_category(cat1);
        report.add_category(cat2);
        report.finalize();

        assert_eq!(report.category_count, 2);
        assert_eq!(report.test_count, 4);
        assert_eq!(report.passed_count, 3);
        assert_eq!(report.failed_count, 1);
        assert!(!report.passed);
        assert_eq!(report.duration_ms, 70);
    }

    #[test]
    fn test_exit_code() {
        let mut report = ConformanceReport::new();
        report.passed = true;
        assert_eq!(report.exit_code(), 0);

        report.passed = false;
        assert_eq!(report.exit_code(), 1);
    }
}
