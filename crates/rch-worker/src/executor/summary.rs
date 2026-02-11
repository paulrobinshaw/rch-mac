//! Agent-friendly summaries (test_summary.json, build_summary.json, junit.xml)
//!
//! Per bead t7z: Worker emits agent-friendly summaries derived from authoritative
//! sources (xcresult when present, logs as fallback).

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

use chrono::Utc;
use regex_lite::Regex;
use serde::{Deserialize, Serialize};

/// Schema version for test_summary.json
pub const TEST_SUMMARY_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for test_summary.json
pub const TEST_SUMMARY_SCHEMA_ID: &str = "rch-xcode/test_summary@1";

/// Schema version for build_summary.json
pub const BUILD_SUMMARY_SCHEMA_VERSION: u32 = 1;
/// Schema identifier for build_summary.json
pub const BUILD_SUMMARY_SCHEMA_ID: &str = "rch-xcode/build_summary@1";

/// Test summary containing test counts, failures, and duration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSummary {
    pub schema_version: u32,
    pub schema_id: String,
    pub run_id: String,
    pub job_id: String,
    pub job_key: String,
    pub created_at: String,

    /// Total number of tests
    pub total_count: u32,
    /// Number of passed tests
    pub passed_count: u32,
    /// Number of failed tests
    pub failed_count: u32,
    /// Number of skipped tests
    pub skipped_count: u32,
    /// Total duration in seconds
    pub duration_seconds: f64,

    /// List of failing tests (limited to top N)
    pub failing_tests: Vec<FailingTest>,
    /// Source of this summary: "xcresult" or "log"
    pub source: String,
}

/// A failing test with details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailingTest {
    /// Test target (e.g., "MyAppTests")
    pub target: String,
    /// Test class (e.g., "LoginTests")
    pub class: String,
    /// Test method (e.g., "testLoginFailure")
    pub method: String,
    /// Duration in seconds (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// Failure message (truncated if long)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// File location (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Line number (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Build summary containing targets, warnings, errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSummary {
    pub schema_version: u32,
    pub schema_id: String,
    pub run_id: String,
    pub job_id: String,
    pub job_key: String,
    pub created_at: String,

    /// List of built targets
    pub targets: Vec<TargetSummary>,
    /// Total warning count
    pub warning_count: u32,
    /// Total error count
    pub error_count: u32,

    /// First error location (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_error: Option<ErrorLocation>,
    /// Top warnings (limited)
    pub top_warnings: Vec<WarningInfo>,
    /// Source of this summary: "xcresult" or "log"
    pub source: String,
}

/// Summary for a built target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSummary {
    /// Target name
    pub name: String,
    /// Whether it succeeded
    pub succeeded: bool,
    /// Number of warnings for this target
    pub warning_count: u32,
    /// Number of errors for this target
    pub error_count: u32,
}

/// Location of an error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorLocation {
    /// File path (relative or absolute)
    pub file: String,
    /// Line number
    pub line: u32,
    /// Column (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Error message (truncated if long)
    pub message: String,
}

/// Warning information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarningInfo {
    /// File path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Line number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Warning message
    pub message: String,
}

/// Maximum number of failing tests to include in summary
const MAX_FAILING_TESTS: usize = 20;
/// Maximum number of warnings to include in summary
const MAX_WARNINGS: usize = 10;
/// Maximum length for error/warning messages
const MAX_MESSAGE_LENGTH: usize = 500;

impl TestSummary {
    /// Create an empty test summary
    pub fn empty(run_id: &str, job_id: &str, job_key: &str) -> Self {
        Self {
            schema_version: TEST_SUMMARY_SCHEMA_VERSION,
            schema_id: TEST_SUMMARY_SCHEMA_ID.to_string(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            created_at: Utc::now().to_rfc3339(),
            total_count: 0,
            passed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            duration_seconds: 0.0,
            failing_tests: vec![],
            source: "none".to_string(),
        }
    }
}

impl BuildSummary {
    /// Create an empty build summary
    pub fn empty(run_id: &str, job_id: &str, job_key: &str) -> Self {
        Self {
            schema_version: BUILD_SUMMARY_SCHEMA_VERSION,
            schema_id: BUILD_SUMMARY_SCHEMA_ID.to_string(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            job_key: job_key.to_string(),
            created_at: Utc::now().to_rfc3339(),
            targets: vec![],
            warning_count: 0,
            error_count: 0,
            first_error: None,
            top_warnings: vec![],
            source: "none".to_string(),
        }
    }
}

/// Generate agent-friendly summaries for a completed job.
///
/// This function attempts to extract information from:
/// 1. xcresult bundle (authoritative for test jobs)
/// 2. build.log (fallback)
///
/// It generates:
/// - test_summary.json (for test jobs)
/// - build_summary.json (for all jobs)
/// - junit.xml (for test jobs)
pub fn generate_summaries(
    artifact_dir: &Path,
    run_id: &str,
    job_id: &str,
    job_key: &str,
    action: &str,
) -> std::io::Result<()> {
    let xcresult_path = artifact_dir.join("result.xcresult");
    let log_path = artifact_dir.join("build.log");

    // Generate test summary and junit.xml for test jobs
    if action == "test" {
        let test_summary = if xcresult_path.exists() {
            parse_xcresult_tests(&xcresult_path, run_id, job_id, job_key)
                .unwrap_or_else(|_| parse_log_tests(&log_path, run_id, job_id, job_key))
        } else {
            parse_log_tests(&log_path, run_id, job_id, job_key)
        };

        write_test_summary(artifact_dir, &test_summary)?;
        write_junit_xml(artifact_dir, &test_summary)?;
    }

    // Generate build summary for all jobs
    let build_summary = if xcresult_path.exists() {
        parse_xcresult_build(&xcresult_path, run_id, job_id, job_key)
            .unwrap_or_else(|_| parse_log_build(&log_path, run_id, job_id, job_key))
    } else {
        parse_log_build(&log_path, run_id, job_id, job_key)
    };

    write_build_summary(artifact_dir, &build_summary)?;

    Ok(())
}

/// Parse test results from xcresult bundle using xcresulttool
fn parse_xcresult_tests(
    xcresult_path: &Path,
    run_id: &str,
    job_id: &str,
    job_key: &str,
) -> Result<TestSummary, std::io::Error> {
    // Use xcresulttool to get test summary
    let output = Command::new("xcrun")
        .args([
            "xcresulttool",
            "get",
            "--format",
            "json",
            "--path",
            xcresult_path.to_str().unwrap(),
        ])
        .output()?;

    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "xcresulttool failed",
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    // Parse the xcresult JSON structure
    let mut summary = TestSummary::empty(run_id, job_id, job_key);
    summary.source = "xcresult".to_string();

    // Extract test counts from the metrics or actions
    if let Some(metrics) = json.get("metrics") {
        if let Some(tests_count) = metrics.get("testsCount").and_then(|v| v.get("_value")) {
            summary.total_count = tests_count.as_u64().unwrap_or(0) as u32;
        }
        if let Some(tests_failed) = metrics.get("testsFailedCount").and_then(|v| v.get("_value")) {
            summary.failed_count = tests_failed.as_u64().unwrap_or(0) as u32;
        }
        if let Some(tests_skipped) = metrics.get("testsSkippedCount").and_then(|v| v.get("_value"))
        {
            summary.skipped_count = tests_skipped.as_u64().unwrap_or(0) as u32;
        }
    }

    // Calculate passed count
    summary.passed_count = summary
        .total_count
        .saturating_sub(summary.failed_count)
        .saturating_sub(summary.skipped_count);

    // Try to extract failing tests from actions
    if let Some(actions) = json.get("actions").and_then(|v| v.get("_values")) {
        if let Some(actions_arr) = actions.as_array() {
            for action in actions_arr {
                extract_failing_tests_from_action(action, &mut summary.failing_tests);
            }
        }
    }

    // Limit failing tests
    summary.failing_tests.truncate(MAX_FAILING_TESTS);

    Ok(summary)
}

/// Extract failing tests from an xcresult action
fn extract_failing_tests_from_action(action: &serde_json::Value, failing_tests: &mut Vec<FailingTest>) {
    // Navigate to test plan run summaries
    if let Some(run_summaries) = action
        .get("actionResult")
        .and_then(|ar| ar.get("testsRef"))
        .and_then(|tr| tr.get("id"))
        .and_then(|id| id.get("_value"))
    {
        // For full test details, we'd need to make another xcresulttool call
        // with the reference ID. For now, we'll rely on the summary data.
        let _ = run_summaries; // Placeholder
    }

    // Extract from testFailureSummaries if available
    if let Some(failure_summaries) = action
        .get("actionResult")
        .and_then(|ar| ar.get("testFailureSummaries"))
        .and_then(|tfs| tfs.get("_values"))
        .and_then(|v| v.as_array())
    {
        for failure in failure_summaries {
            if let Some(test_name) = failure.get("testCaseName").and_then(|n| n.get("_value")).and_then(|v| v.as_str()) {
                // Parse test name format: "TargetName.ClassName/testMethod()"
                let parts: Vec<&str> = test_name.split('.').collect();
                let (target, class_method) = if parts.len() >= 2 {
                    (parts[0].to_string(), parts[1..].join("."))
                } else {
                    ("Unknown".to_string(), test_name.to_string())
                };

                let (class, method) = if let Some(slash_idx) = class_method.find('/') {
                    (
                        class_method[..slash_idx].to_string(),
                        class_method[slash_idx + 1..].trim_end_matches("()").to_string(),
                    )
                } else {
                    ("Unknown".to_string(), class_method)
                };

                let message = failure
                    .get("message")
                    .and_then(|m| m.get("_value"))
                    .and_then(|v| v.as_str())
                    .map(|s| truncate_message(s, MAX_MESSAGE_LENGTH));

                failing_tests.push(FailingTest {
                    target,
                    class,
                    method,
                    duration_seconds: None,
                    message,
                    file: None,
                    line: None,
                });
            }
        }
    }
}

/// Parse test results from build.log as fallback
fn parse_log_tests(log_path: &Path, run_id: &str, job_id: &str, job_key: &str) -> TestSummary {
    let mut summary = TestSummary::empty(run_id, job_id, job_key);
    summary.source = "log".to_string();

    let file = match File::open(log_path) {
        Ok(f) => f,
        Err(_) => return summary,
    };
    let reader = BufReader::new(file);

    // Regex patterns for test output
    // Pattern: "Test Case '-[TargetName.ClassName testMethod]' passed/failed (X.XXX seconds)."
    let test_case_re =
        Regex::new(r"Test Case '-\[(\w+)\.(\w+) (\w+)\]' (passed|failed) \((\d+\.\d+) seconds\)")
            .unwrap();

    // Pattern: "Executed N tests, with M failures in X.XXX seconds"
    let summary_re = Regex::new(r"Executed (\d+) tests?, with (\d+) failures? .* in (\d+\.\d+)").unwrap();

    // Pattern for Swift test format: "◇ Test testMethod() started"
    // "✔ Test testMethod() passed after X.XXX seconds"
    // "✘ Test testMethod() failed after X.XXX seconds"
    let swift_test_re =
        Regex::new(r"[✔✘◇] Test (\w+)\(\) (passed|failed|started)(?: after (\d+\.\d+) seconds)?")
            .unwrap();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Check for test case results (ObjC format)
        if let Some(caps) = test_case_re.captures(&line) {
            let target = caps.get(1).map(|m| m.as_str()).unwrap_or("Unknown");
            let class = caps.get(2).map(|m| m.as_str()).unwrap_or("Unknown");
            let method = caps.get(3).map(|m| m.as_str()).unwrap_or("Unknown");
            let result = caps.get(4).map(|m| m.as_str()).unwrap_or("unknown");
            let duration: f64 = caps
                .get(5)
                .map(|m| m.as_str().parse().unwrap_or(0.0))
                .unwrap_or(0.0);

            summary.total_count += 1;
            if result == "passed" {
                summary.passed_count += 1;
            } else if result == "failed" {
                summary.failed_count += 1;
                if summary.failing_tests.len() < MAX_FAILING_TESTS {
                    summary.failing_tests.push(FailingTest {
                        target: target.to_string(),
                        class: class.to_string(),
                        method: method.to_string(),
                        duration_seconds: Some(duration),
                        message: None,
                        file: None,
                        line: None,
                    });
                }
            }
        }

        // Check for Swift test format
        if let Some(caps) = swift_test_re.captures(&line) {
            let method = caps.get(1).map(|m| m.as_str()).unwrap_or("Unknown");
            let result = caps.get(2).map(|m| m.as_str()).unwrap_or("unknown");
            let duration: Option<f64> = caps.get(3).map(|m| m.as_str().parse().unwrap_or(0.0));

            if result != "started" {
                summary.total_count += 1;
                if result == "passed" {
                    summary.passed_count += 1;
                } else if result == "failed" {
                    summary.failed_count += 1;
                    if summary.failing_tests.len() < MAX_FAILING_TESTS {
                        summary.failing_tests.push(FailingTest {
                            target: "Unknown".to_string(),
                            class: "Unknown".to_string(),
                            method: method.to_string(),
                            duration_seconds: duration,
                            message: None,
                            file: None,
                            line: None,
                        });
                    }
                }
            }
        }

        // Check for test summary line
        if let Some(caps) = summary_re.captures(&line) {
            // Prefer the summary line counts if found
            let total: u32 = caps.get(1).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            let failed: u32 = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            let duration: f64 = caps.get(3).map(|m| m.as_str().parse().unwrap_or(0.0)).unwrap_or(0.0);

            // Only use if we didn't get counts from individual tests
            if summary.total_count == 0 {
                summary.total_count = total;
                summary.failed_count = failed;
                summary.passed_count = total.saturating_sub(failed);
            }
            summary.duration_seconds = duration;
        }
    }

    summary
}

/// Parse build results from xcresult bundle
fn parse_xcresult_build(
    xcresult_path: &Path,
    run_id: &str,
    job_id: &str,
    job_key: &str,
) -> Result<BuildSummary, std::io::Error> {
    let output = Command::new("xcrun")
        .args([
            "xcresulttool",
            "get",
            "--format",
            "json",
            "--path",
            xcresult_path.to_str().unwrap(),
        ])
        .output()?;

    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "xcresulttool failed",
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut summary = BuildSummary::empty(run_id, job_id, job_key);
    summary.source = "xcresult".to_string();

    // Extract metrics
    if let Some(metrics) = json.get("metrics") {
        if let Some(warnings) = metrics.get("warningCount").and_then(|v| v.get("_value")) {
            summary.warning_count = warnings.as_u64().unwrap_or(0) as u32;
        }
        if let Some(errors) = metrics.get("errorCount").and_then(|v| v.get("_value")) {
            summary.error_count = errors.as_u64().unwrap_or(0) as u32;
        }
    }

    // Extract issues (warnings and errors)
    if let Some(issues) = json.get("issues") {
        // Extract errors
        if let Some(error_summaries) = issues
            .get("errorSummaries")
            .and_then(|es| es.get("_values"))
            .and_then(|v| v.as_array())
        {
            for (i, error) in error_summaries.iter().enumerate() {
                if i == 0 {
                    // First error
                    let message = error
                        .get("message")
                        .and_then(|m| m.get("_value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown error");

                    let file_loc = error
                        .get("documentLocationInCreatingWorkspace")
                        .and_then(|d| d.get("url"))
                        .and_then(|u| u.get("_value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Parse file:line from URL
                    let (file, line) = parse_file_url(file_loc);

                    summary.first_error = Some(ErrorLocation {
                        file,
                        line,
                        column: None,
                        message: truncate_message(message, MAX_MESSAGE_LENGTH),
                    });
                }
            }
        }

        // Extract warnings
        if let Some(warning_summaries) = issues
            .get("warningSummaries")
            .and_then(|ws| ws.get("_values"))
            .and_then(|v| v.as_array())
        {
            for warning in warning_summaries.iter().take(MAX_WARNINGS) {
                let message = warning
                    .get("message")
                    .and_then(|m| m.get("_value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown warning");

                let file_loc = warning
                    .get("documentLocationInCreatingWorkspace")
                    .and_then(|d| d.get("url"))
                    .and_then(|u| u.get("_value"))
                    .and_then(|v| v.as_str());

                let (file, line) = file_loc.map(parse_file_url).unwrap_or(("".to_string(), 0));

                summary.top_warnings.push(WarningInfo {
                    file: if file.is_empty() { None } else { Some(file) },
                    line: if line > 0 { Some(line) } else { None },
                    message: truncate_message(message, MAX_MESSAGE_LENGTH),
                });
            }
        }
    }

    Ok(summary)
}

/// Parse file:line from xcresult URL format
fn parse_file_url(url: &str) -> (String, u32) {
    // Format: file:///path/to/file.swift#StartingLineNumber=42&...
    let path = url
        .strip_prefix("file://")
        .unwrap_or(url)
        .split('#')
        .next()
        .unwrap_or(url)
        .to_string();

    let line = if let Some(fragment) = url.split('#').nth(1) {
        // Parse StartingLineNumber=X
        fragment
            .split('&')
            .find(|p| p.starts_with("StartingLineNumber="))
            .and_then(|p| p.strip_prefix("StartingLineNumber="))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    } else {
        0
    };

    (path, line)
}

/// Parse build results from build.log as fallback
fn parse_log_build(log_path: &Path, run_id: &str, job_id: &str, job_key: &str) -> BuildSummary {
    let mut summary = BuildSummary::empty(run_id, job_id, job_key);
    summary.source = "log".to_string();

    let file = match File::open(log_path) {
        Ok(f) => f,
        Err(_) => return summary,
    };
    let reader = BufReader::new(file);

    // Regex patterns for build output
    // Error: /path/to/file.swift:123:45: error: message
    let error_re = Regex::new(r"^([^:]+):(\d+):(\d+):\s*error:\s*(.+)$").unwrap();
    // Warning: /path/to/file.swift:123:45: warning: message
    let warning_re = Regex::new(r"^([^:]+):(\d+):(\d+):\s*warning:\s*(.+)$").unwrap();
    // Target build status: Build target X (project Y)
    let target_re = Regex::new(r"Build target (\w+)").unwrap();
    // Target success: Target X: build succeeded
    let target_success_re = Regex::new(r"Target (\w+):.*(succeeded|failed)").unwrap();

    let mut current_targets: std::collections::HashMap<String, TargetSummary> =
        std::collections::HashMap::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Check for errors
        if let Some(caps) = error_re.captures(&line) {
            summary.error_count += 1;

            if summary.first_error.is_none() {
                let file = caps.get(1).map(|m| m.as_str()).unwrap_or("unknown");
                let line_num: u32 = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
                let col: u32 = caps.get(3).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
                let message = caps.get(4).map(|m| m.as_str()).unwrap_or("Unknown error");

                summary.first_error = Some(ErrorLocation {
                    file: file.to_string(),
                    line: line_num,
                    column: Some(col),
                    message: truncate_message(message, MAX_MESSAGE_LENGTH),
                });
            }
        }

        // Check for warnings
        if let Some(caps) = warning_re.captures(&line) {
            summary.warning_count += 1;

            if summary.top_warnings.len() < MAX_WARNINGS {
                let file = caps.get(1).map(|m| m.as_str()).unwrap_or("unknown");
                let line_num: u32 = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
                let message = caps.get(4).map(|m| m.as_str()).unwrap_or("Unknown warning");

                summary.top_warnings.push(WarningInfo {
                    file: Some(file.to_string()),
                    line: Some(line_num),
                    message: truncate_message(message, MAX_MESSAGE_LENGTH),
                });
            }
        }

        // Track target builds
        if let Some(caps) = target_re.captures(&line) {
            let target_name = caps.get(1).map(|m| m.as_str()).unwrap_or("Unknown");
            current_targets.entry(target_name.to_string()).or_insert(TargetSummary {
                name: target_name.to_string(),
                succeeded: false, // Updated later
                warning_count: 0,
                error_count: 0,
            });
        }

        // Track target success/failure
        if let Some(caps) = target_success_re.captures(&line) {
            let target_name = caps.get(1).map(|m| m.as_str()).unwrap_or("Unknown");
            let result = caps.get(2).map(|m| m.as_str()).unwrap_or("unknown");

            if let Some(target) = current_targets.get_mut(target_name) {
                target.succeeded = result == "succeeded";
            }
        }
    }

    summary.targets = current_targets.into_values().collect();

    summary
}

/// Write test_summary.json to artifact directory
fn write_test_summary(artifact_dir: &Path, summary: &TestSummary) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(summary)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let final_path = artifact_dir.join("test_summary.json");
    let temp_path = artifact_dir.join(".test_summary.json.tmp");

    fs::write(&temp_path, &json)?;
    fs::rename(&temp_path, &final_path)?;

    Ok(())
}

/// Write build_summary.json to artifact directory
fn write_build_summary(artifact_dir: &Path, summary: &BuildSummary) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(summary)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let final_path = artifact_dir.join("build_summary.json");
    let temp_path = artifact_dir.join(".build_summary.json.tmp");

    fs::write(&temp_path, &json)?;
    fs::rename(&temp_path, &final_path)?;

    Ok(())
}

/// Write junit.xml to artifact directory
fn write_junit_xml(artifact_dir: &Path, summary: &TestSummary) -> std::io::Result<()> {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

    // Root testsuites element
    xml.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        summary.total_count,
        summary.failed_count,
        summary.skipped_count,
        summary.duration_seconds
    ));

    // Group tests by target.class
    let mut test_classes: std::collections::HashMap<String, Vec<&FailingTest>> =
        std::collections::HashMap::new();

    for test in &summary.failing_tests {
        let key = format!("{}.{}", test.target, test.class);
        test_classes.entry(key).or_default().push(test);
    }

    // Write testsuite for each class (for failing tests)
    for (class_name, tests) in &test_classes {
        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\">\n",
            xml_escape(class_name),
            tests.len(),
            tests.len()
        ));

        for test in tests {
            xml.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}.{}\" time=\"{:.3}\">\n",
                xml_escape(&test.method),
                xml_escape(&test.target),
                xml_escape(&test.class),
                test.duration_seconds.unwrap_or(0.0)
            ));

            // Add failure element
            xml.push_str("      <failure type=\"TestFailure\"");
            if let Some(ref msg) = test.message {
                xml.push_str(&format!(" message=\"{}\"", xml_escape(msg)));
            }
            xml.push_str(">");

            // Add file/line as content if available
            if let Some(ref file) = test.file {
                xml.push_str(&xml_escape(file));
                if let Some(line) = test.line {
                    xml.push_str(&format!(":{}", line));
                }
            }

            xml.push_str("</failure>\n");
            xml.push_str("    </testcase>\n");
        }

        xml.push_str("  </testsuite>\n");
    }

    // If we have passed tests but no failures tracked, add a summary testsuite
    if summary.passed_count > 0 && summary.failing_tests.is_empty() && test_classes.is_empty() {
        xml.push_str(&format!(
            "  <testsuite name=\"Tests\" tests=\"{}\" failures=\"{}\" skipped=\"{}\">\n",
            summary.total_count, summary.failed_count, summary.skipped_count
        ));
        // Add placeholder passed testcase
        xml.push_str("    <testcase name=\"all_tests\" classname=\"TestSuite\" />\n");
        xml.push_str("  </testsuite>\n");
    }

    xml.push_str("</testsuites>\n");

    let final_path = artifact_dir.join("junit.xml");
    let temp_path = artifact_dir.join(".junit.xml.tmp");

    fs::write(&temp_path, &xml)?;
    fs::rename(&temp_path, &final_path)?;

    Ok(())
}

/// XML-escape a string
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Truncate a message to max length
fn truncate_message(msg: &str, max_len: usize) -> String {
    if msg.len() <= max_len {
        msg.to_string()
    } else {
        format!("{}...", &msg[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_empty_test_summary() {
        let summary = TestSummary::empty("run-001", "job-001", "abc123");
        assert_eq!(summary.total_count, 0);
        assert_eq!(summary.passed_count, 0);
        assert_eq!(summary.failed_count, 0);
        assert_eq!(summary.source, "none");
        assert_eq!(summary.schema_version, TEST_SUMMARY_SCHEMA_VERSION);
    }

    #[test]
    fn test_empty_build_summary() {
        let summary = BuildSummary::empty("run-001", "job-001", "abc123");
        assert_eq!(summary.warning_count, 0);
        assert_eq!(summary.error_count, 0);
        assert!(summary.first_error.is_none());
        assert_eq!(summary.source, "none");
        assert_eq!(summary.schema_version, BUILD_SUMMARY_SCHEMA_VERSION);
    }

    #[test]
    fn test_parse_log_tests() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("build.log");

        // Write a sample test log
        fs::write(
            &log_path,
            r#"
Test Case '-[MyAppTests.LoginTests testLoginSuccess]' passed (0.123 seconds).
Test Case '-[MyAppTests.LoginTests testLoginFailure]' failed (0.456 seconds).
Test Case '-[MyAppTests.SignupTests testSignup]' passed (0.789 seconds).
Executed 3 tests, with 1 failure in 1.368 seconds.
"#,
        )
        .unwrap();

        let summary = parse_log_tests(&log_path, "run-001", "job-001", "abc123");

        assert_eq!(summary.total_count, 3);
        assert_eq!(summary.passed_count, 2);
        assert_eq!(summary.failed_count, 1);
        assert_eq!(summary.failing_tests.len(), 1);
        assert_eq!(summary.failing_tests[0].target, "MyAppTests");
        assert_eq!(summary.failing_tests[0].class, "LoginTests");
        assert_eq!(summary.failing_tests[0].method, "testLoginFailure");
        assert_eq!(summary.source, "log");
    }

    #[test]
    fn test_parse_log_build() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("build.log");

        // Write a sample build log with errors and warnings
        fs::write(
            &log_path,
            r#"
Build target MyApp (project MyApp)
/path/to/file.swift:42:10: warning: unused variable 'x'
/path/to/other.swift:100:5: error: cannot find 'foo' in scope
/path/to/file.swift:50:10: warning: deprecated function
Target MyApp: build failed
"#,
        )
        .unwrap();

        let summary = parse_log_build(&log_path, "run-001", "job-001", "abc123");

        assert_eq!(summary.warning_count, 2);
        assert_eq!(summary.error_count, 1);
        assert!(summary.first_error.is_some());

        let first_error = summary.first_error.as_ref().unwrap();
        assert_eq!(first_error.file, "/path/to/other.swift");
        assert_eq!(first_error.line, 100);
        assert!(first_error.message.contains("cannot find 'foo'"));
        assert_eq!(summary.source, "log");
    }

    #[test]
    fn test_write_test_summary() {
        let temp_dir = TempDir::new().unwrap();
        let artifact_dir = temp_dir.path();

        let mut summary = TestSummary::empty("run-001", "job-001", "abc123");
        summary.total_count = 10;
        summary.passed_count = 8;
        summary.failed_count = 2;
        summary.source = "log".to_string();
        summary.failing_tests.push(FailingTest {
            target: "MyApp".to_string(),
            class: "TestClass".to_string(),
            method: "testMethod".to_string(),
            duration_seconds: Some(0.5),
            message: Some("Expected true but got false".to_string()),
            file: None,
            line: None,
        });

        write_test_summary(artifact_dir, &summary).unwrap();

        let path = artifact_dir.join("test_summary.json");
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["total_count"], 10);
        assert_eq!(parsed["passed_count"], 8);
        assert_eq!(parsed["failed_count"], 2);
        assert_eq!(parsed["schema_id"], TEST_SUMMARY_SCHEMA_ID);
    }

    #[test]
    fn test_write_build_summary() {
        let temp_dir = TempDir::new().unwrap();
        let artifact_dir = temp_dir.path();

        let mut summary = BuildSummary::empty("run-001", "job-001", "abc123");
        summary.warning_count = 5;
        summary.error_count = 1;
        summary.source = "log".to_string();
        summary.first_error = Some(ErrorLocation {
            file: "/path/to/file.swift".to_string(),
            line: 42,
            column: Some(10),
            message: "Type mismatch".to_string(),
        });

        write_build_summary(artifact_dir, &summary).unwrap();

        let path = artifact_dir.join("build_summary.json");
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed["warning_count"], 5);
        assert_eq!(parsed["error_count"], 1);
        assert_eq!(parsed["first_error"]["file"], "/path/to/file.swift");
        assert_eq!(parsed["first_error"]["line"], 42);
        assert_eq!(parsed["schema_id"], BUILD_SUMMARY_SCHEMA_ID);
    }

    #[test]
    fn test_write_junit_xml() {
        let temp_dir = TempDir::new().unwrap();
        let artifact_dir = temp_dir.path();

        let mut summary = TestSummary::empty("run-001", "job-001", "abc123");
        summary.total_count = 3;
        summary.passed_count = 2;
        summary.failed_count = 1;
        summary.duration_seconds = 1.5;
        summary.failing_tests.push(FailingTest {
            target: "MyApp".to_string(),
            class: "LoginTests".to_string(),
            method: "testLoginFailure".to_string(),
            duration_seconds: Some(0.5),
            message: Some("Login failed unexpectedly".to_string()),
            file: Some("/path/to/test.swift".to_string()),
            line: Some(42),
        });

        write_junit_xml(artifact_dir, &summary).unwrap();

        let path = artifact_dir.join("junit.xml");
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("<?xml version=\"1.0\""));
        assert!(content.contains("<testsuites"));
        assert!(content.contains("tests=\"3\""));
        assert!(content.contains("failures=\"1\""));
        assert!(content.contains("testLoginFailure"));
        assert!(content.contains("Login failed unexpectedly"));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("hello"), "hello");
        assert_eq!(xml_escape("<test>"), "&lt;test&gt;");
        assert_eq!(xml_escape("foo & bar"), "foo &amp; bar");
        assert_eq!(xml_escape("say \"hello\""), "say &quot;hello&quot;");
    }

    #[test]
    fn test_truncate_message() {
        assert_eq!(truncate_message("short", 100), "short");
        assert_eq!(truncate_message("this is a longer message", 10), "this is...");
    }

    #[test]
    fn test_parse_file_url() {
        let (path, line) = parse_file_url("file:///path/to/file.swift#StartingLineNumber=42&EndingLineNumber=50");
        assert_eq!(path, "/path/to/file.swift");
        assert_eq!(line, 42);

        let (path2, line2) = parse_file_url("/simple/path.swift");
        assert_eq!(path2, "/simple/path.swift");
        assert_eq!(line2, 0);
    }

    #[test]
    fn test_generate_summaries_test_job() {
        let temp_dir = TempDir::new().unwrap();
        let artifact_dir = temp_dir.path();

        // Create a mock build.log
        fs::write(
            artifact_dir.join("build.log"),
            r#"
Test Case '-[MyAppTests.Tests testExample]' passed (0.1 seconds).
Executed 1 test, with 0 failures in 0.1 seconds.
"#,
        )
        .unwrap();

        generate_summaries(artifact_dir, "run-001", "job-001", "abc123", "test").unwrap();

        // Check test_summary.json exists
        assert!(artifact_dir.join("test_summary.json").exists());
        // Check junit.xml exists
        assert!(artifact_dir.join("junit.xml").exists());
        // Check build_summary.json exists
        assert!(artifact_dir.join("build_summary.json").exists());
    }

    #[test]
    fn test_generate_summaries_build_job() {
        let temp_dir = TempDir::new().unwrap();
        let artifact_dir = temp_dir.path();

        // Create a mock build.log
        fs::write(artifact_dir.join("build.log"), "Build succeeded\n").unwrap();

        generate_summaries(artifact_dir, "run-001", "job-001", "abc123", "build").unwrap();

        // Check build_summary.json exists
        assert!(artifact_dir.join("build_summary.json").exists());
        // test_summary.json should NOT exist for build jobs
        assert!(!artifact_dir.join("test_summary.json").exists());
        // junit.xml should NOT exist for build jobs
        assert!(!artifact_dir.join("junit.xml").exists());
    }
}
