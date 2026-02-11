//! Schema versioning and compatibility rules for RCH Xcode Lane artifacts.
//!
//! Per PLAN.md (Artifact schemas + versioning section):
//! - All JSON artifacts MUST include: schema_version, schema_id, created_at
//! - Run-scoped artifacts MUST include: run_id
//! - Job-scoped artifacts MUST include: run_id, job_id, job_key
//! - Schema IDs use format: `rch-xcode/<artifact-type>@<major-version>`
//!
//! ## Schema compatibility rules
//! - Adding new optional fields is a backward-compatible change (no version bump)
//! - Removing fields, changing meanings/types, or tightening constraints requires version bump
//!
//! ## Unknown schema version handling
//! - If schema_id major version matches: parse known fields, ignore unknown (forward-compatible)
//! - If schema_id major version differs: reject with diagnostic naming expected vs actual

use serde::{Deserialize, Serialize};
use std::fmt;

/// Error type for schema validation failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The schema_id format is invalid (missing @, invalid major version).
    InvalidFormat {
        schema_id: String,
        reason: String,
    },
    /// The schema_id type prefix doesn't match expected.
    TypeMismatch {
        expected_prefix: String,
        actual_prefix: String,
        schema_id: String,
    },
    /// The schema_id major version differs from expected.
    MajorVersionMismatch {
        expected: u32,
        actual: u32,
        expected_schema_id: String,
        actual_schema_id: String,
    },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::InvalidFormat { schema_id, reason } => {
                write!(f, "invalid schema_id '{}': {}", schema_id, reason)
            }
            SchemaError::TypeMismatch {
                expected_prefix,
                actual_prefix,
                schema_id,
            } => {
                write!(
                    f,
                    "schema type mismatch: expected prefix '{}', got '{}' in schema_id '{}'",
                    expected_prefix, actual_prefix, schema_id
                )
            }
            SchemaError::MajorVersionMismatch {
                expected,
                actual,
                expected_schema_id,
                actual_schema_id,
            } => {
                write!(
                    f,
                    "schema major version mismatch: expected {} ({}), got {} ({})",
                    expected, expected_schema_id, actual, actual_schema_id
                )
            }
        }
    }
}

impl std::error::Error for SchemaError {}

/// Parsed schema identifier components.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaId {
    /// Full schema_id string (e.g., "rch-xcode/summary@1")
    pub full: String,
    /// Prefix/type portion (e.g., "rch-xcode/summary")
    pub prefix: String,
    /// Major version number extracted from the @N suffix
    pub major_version: u32,
}

impl SchemaId {
    /// Parse a schema_id string into its components.
    ///
    /// Expected format: `<prefix>@<major-version>`
    /// Example: `rch-xcode/summary@1` -> prefix="rch-xcode/summary", major_version=1
    pub fn parse(schema_id: &str) -> Result<Self, SchemaError> {
        let at_pos = schema_id.rfind('@').ok_or_else(|| SchemaError::InvalidFormat {
            schema_id: schema_id.to_string(),
            reason: "missing '@' delimiter before version number".to_string(),
        })?;

        let prefix = &schema_id[..at_pos];
        let version_str = &schema_id[at_pos + 1..];

        if prefix.is_empty() {
            return Err(SchemaError::InvalidFormat {
                schema_id: schema_id.to_string(),
                reason: "empty prefix before '@'".to_string(),
            });
        }

        let major_version: u32 =
            version_str
                .parse()
                .map_err(|_| SchemaError::InvalidFormat {
                    schema_id: schema_id.to_string(),
                    reason: format!("invalid major version '{}', expected integer", version_str),
                })?;

        Ok(Self {
            full: schema_id.to_string(),
            prefix: prefix.to_string(),
            major_version,
        })
    }
}

/// Validate that an artifact's schema is compatible with the expected schema.
///
/// Per PLAN.md unknown schema version handling:
/// - If schema_id major version matches: forward-compatible (parse known fields, ignore unknown)
/// - If schema_id major version differs: reject with diagnostic
///
/// # Arguments
/// * `expected_schema_id` - The schema_id this consumer expects (e.g., "rch-xcode/summary@1")
/// * `actual_schema_id` - The schema_id from the artifact being loaded
///
/// # Returns
/// * `Ok(())` if compatible (major versions match)
/// * `Err(SchemaError)` if incompatible
pub fn validate_schema_compatibility(
    expected_schema_id: &str,
    actual_schema_id: &str,
) -> Result<(), SchemaError> {
    let expected = SchemaId::parse(expected_schema_id)?;
    let actual = SchemaId::parse(actual_schema_id)?;

    // Check that the prefix/type matches
    if expected.prefix != actual.prefix {
        return Err(SchemaError::TypeMismatch {
            expected_prefix: expected.prefix,
            actual_prefix: actual.prefix,
            schema_id: actual_schema_id.to_string(),
        });
    }

    // Check major version compatibility
    if expected.major_version != actual.major_version {
        return Err(SchemaError::MajorVersionMismatch {
            expected: expected.major_version,
            actual: actual.major_version,
            expected_schema_id: expected_schema_id.to_string(),
            actual_schema_id: actual_schema_id.to_string(),
        });
    }

    Ok(())
}

/// Check if an artifact schema is forward-compatible.
///
/// Returns true if:
/// - The schema_id major version matches (schema_version can differ)
/// - The artifact may contain unknown fields that should be ignored
pub fn is_forward_compatible(expected_schema_id: &str, actual_schema_id: &str) -> bool {
    validate_schema_compatibility(expected_schema_id, actual_schema_id).is_ok()
}

/// Common artifact header fields per PLAN.md.
///
/// All JSON artifacts MUST include these fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactHeader {
    pub schema_version: u32,
    pub schema_id: String,
    pub created_at: String,
}

/// Run-scoped artifact header (includes run_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunScopedHeader {
    pub schema_version: u32,
    pub schema_id: String,
    pub created_at: String,
    pub run_id: String,
}

/// Job-scoped artifact header (includes run_id, job_id, job_key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobScopedHeader {
    pub schema_version: u32,
    pub schema_id: String,
    pub created_at: String,
    pub run_id: String,
    pub job_id: String,
    pub job_key: String,
}

/// Extract and validate the artifact header from JSON.
///
/// This is useful for checking schema compatibility before full deserialization.
pub fn extract_and_validate_header(
    json: &str,
    expected_schema_id: &str,
) -> Result<ArtifactHeader, SchemaError> {
    // Parse just enough to get the header fields
    let header: ArtifactHeader = serde_json::from_str(json).map_err(|e| {
        SchemaError::InvalidFormat {
            schema_id: "".to_string(),
            reason: format!("failed to parse artifact header: {}", e),
        }
    })?;

    validate_schema_compatibility(expected_schema_id, &header.schema_id)?;

    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_schema_id() {
        let schema = SchemaId::parse("rch-xcode/summary@1").unwrap();
        assert_eq!(schema.prefix, "rch-xcode/summary");
        assert_eq!(schema.major_version, 1);
        assert_eq!(schema.full, "rch-xcode/summary@1");
    }

    #[test]
    fn test_parse_schema_id_higher_version() {
        let schema = SchemaId::parse("rch-xcode/job@2").unwrap();
        assert_eq!(schema.prefix, "rch-xcode/job");
        assert_eq!(schema.major_version, 2);
    }

    #[test]
    fn test_parse_schema_id_invalid_no_at() {
        let result = SchemaId::parse("rch-xcode/summary");
        assert!(matches!(result, Err(SchemaError::InvalidFormat { .. })));
    }

    #[test]
    fn test_parse_schema_id_invalid_version() {
        let result = SchemaId::parse("rch-xcode/summary@abc");
        assert!(matches!(result, Err(SchemaError::InvalidFormat { .. })));
    }

    #[test]
    fn test_parse_schema_id_empty_prefix() {
        let result = SchemaId::parse("@1");
        assert!(matches!(result, Err(SchemaError::InvalidFormat { .. })));
    }

    #[test]
    fn test_validate_schema_compatibility_same_version() {
        let result =
            validate_schema_compatibility("rch-xcode/summary@1", "rch-xcode/summary@1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_schema_compatibility_different_major() {
        let result =
            validate_schema_compatibility("rch-xcode/summary@1", "rch-xcode/summary@2");
        assert!(matches!(
            result,
            Err(SchemaError::MajorVersionMismatch { .. })
        ));
    }

    #[test]
    fn test_validate_schema_compatibility_type_mismatch() {
        let result = validate_schema_compatibility("rch-xcode/summary@1", "rch-xcode/job@1");
        assert!(matches!(result, Err(SchemaError::TypeMismatch { .. })));
    }

    #[test]
    fn test_is_forward_compatible() {
        assert!(is_forward_compatible(
            "rch-xcode/summary@1",
            "rch-xcode/summary@1"
        ));
        assert!(!is_forward_compatible(
            "rch-xcode/summary@1",
            "rch-xcode/summary@2"
        ));
        assert!(!is_forward_compatible(
            "rch-xcode/summary@1",
            "rch-xcode/job@1"
        ));
    }

    #[test]
    fn test_error_display() {
        let err = SchemaError::MajorVersionMismatch {
            expected: 1,
            actual: 2,
            expected_schema_id: "rch-xcode/summary@1".to_string(),
            actual_schema_id: "rch-xcode/summary@2".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("schema major version mismatch"));
        assert!(msg.contains("expected 1"));
        assert!(msg.contains("got 2"));
    }

    #[test]
    fn test_extract_and_validate_header() {
        let json = r#"{"schema_version": 1, "schema_id": "rch-xcode/summary@1", "created_at": "2026-02-11T00:00:00Z", "other_field": "value"}"#;
        let result = extract_and_validate_header(json, "rch-xcode/summary@1");
        assert!(result.is_ok());
        let header = result.unwrap();
        assert_eq!(header.schema_version, 1);
        assert_eq!(header.schema_id, "rch-xcode/summary@1");
    }

    #[test]
    fn test_extract_and_validate_header_version_mismatch() {
        let json = r#"{"schema_version": 2, "schema_id": "rch-xcode/summary@2", "created_at": "2026-02-11T00:00:00Z"}"#;
        let result = extract_and_validate_header(json, "rch-xcode/summary@1");
        assert!(matches!(
            result,
            Err(SchemaError::MajorVersionMismatch { .. })
        ));
    }
}
