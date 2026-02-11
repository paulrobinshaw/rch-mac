//! xcodebuild argument parser
//!
//! Parses xcodebuild command-line arguments into structured form.
//! Handles xcodebuild's flag conventions:
//! - Single-dash flags with space-separated values: `-workspace Foo.xcworkspace`
//! - Actions are bare words: `build`, `test`, `archive`
//! - Some flags are boolean (no value)

use std::collections::HashMap;

/// Parsed xcodebuild arguments
#[derive(Debug, Clone, Default)]
pub struct ParsedXcodebuildArgs {
    /// The action (build, test, archive, etc.) - first bare word
    pub action: Option<String>,
    /// Flag -> Value mapping (value is None for boolean flags)
    pub flags: HashMap<String, Option<String>>,
}

/// Known boolean flags that don't take a value
const BOOLEAN_FLAGS: &[&str] = &[
    "-quiet",
    "-verbose",
    "-json",
    "-enableCodeCoverage",
    "-disableAutomaticPackageResolution",
    "-onlyUsePackageVersionsFromResolvedFile",
    "-skipPackagePluginValidation",
    "-skipMacroValidation",
    "-allowProvisioningUpdates",
    "-allowProvisioningDeviceRegistration",
];

/// Known actions in xcodebuild
const KNOWN_ACTIONS: &[&str] = &[
    "build",
    "build-for-testing",
    "analyze",
    "archive",
    "test",
    "test-without-building",
    "install",
    "clean",
];

/// Parse xcodebuild argv into structured form
///
/// # Arguments
/// * `argv` - The command-line arguments (excluding "xcodebuild" itself if present)
///
/// # Returns
/// * `Ok(ParsedXcodebuildArgs)` on success
/// * `Err(String)` with error message on parse failure
pub fn parse_xcodebuild_argv(argv: &[String]) -> Result<ParsedXcodebuildArgs, String> {
    let mut result = ParsedXcodebuildArgs::default();
    let mut i = 0;

    // Skip "xcodebuild" if it's the first argument
    if !argv.is_empty() && argv[0] == "xcodebuild" {
        i = 1;
    }

    while i < argv.len() {
        let arg = &argv[i];

        if arg.starts_with('-') {
            // This is a flag
            let flag = arg.clone();

            // Check if it's a boolean flag
            if BOOLEAN_FLAGS.contains(&flag.as_str()) {
                result.flags.insert(flag, None);
                i += 1;
                continue;
            }

            // Check if the next argument exists and is not a flag
            if i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
                // Flag with value
                let value = argv[i + 1].clone();
                result.flags.insert(flag, Some(value));
                i += 2;
            } else {
                // Flag without value (might be boolean or error)
                result.flags.insert(flag, None);
                i += 1;
            }
        } else if KNOWN_ACTIONS.contains(&arg.as_str()) {
            // This is an action
            if result.action.is_some() {
                return Err(format!(
                    "Multiple actions specified: {} and {}",
                    result.action.as_ref().unwrap(),
                    arg
                ));
            }
            result.action = Some(arg.clone());
            i += 1;
        } else if arg.contains('=') {
            // Build setting in KEY=VALUE format (e.g., CODE_SIGN_IDENTITY=-)
            // Treat as a special flag
            result.flags.insert(arg.clone(), None);
            i += 1;
        } else {
            // Unknown bare word - could be an unknown action
            // We'll treat the first bare word as an action if none set yet
            if result.action.is_none() {
                result.action = Some(arg.clone());
            } else {
                // Multiple bare words - might be build settings without =
                result.flags.insert(arg.clone(), None);
            }
            i += 1;
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_build() {
        let argv: Vec<String> = vec!["build", "-workspace", "MyApp.xcworkspace", "-scheme", "MyApp"]
            .into_iter()
            .map(String::from)
            .collect();

        let parsed = parse_xcodebuild_argv(&argv).unwrap();

        assert_eq!(parsed.action, Some("build".to_string()));
        assert_eq!(
            parsed.flags.get("-workspace"),
            Some(&Some("MyApp.xcworkspace".to_string()))
        );
        assert_eq!(
            parsed.flags.get("-scheme"),
            Some(&Some("MyApp".to_string()))
        );
    }

    #[test]
    fn test_parse_with_xcodebuild_prefix() {
        let argv: Vec<String> = vec!["xcodebuild", "test", "-scheme", "MyApp"]
            .into_iter()
            .map(String::from)
            .collect();

        let parsed = parse_xcodebuild_argv(&argv).unwrap();

        assert_eq!(parsed.action, Some("test".to_string()));
    }

    #[test]
    fn test_parse_boolean_flag() {
        let argv: Vec<String> = vec!["build", "-quiet", "-scheme", "MyApp"]
            .into_iter()
            .map(String::from)
            .collect();

        let parsed = parse_xcodebuild_argv(&argv).unwrap();

        assert_eq!(parsed.flags.get("-quiet"), Some(&None));
        assert_eq!(
            parsed.flags.get("-scheme"),
            Some(&Some("MyApp".to_string()))
        );
    }

    #[test]
    fn test_parse_destination() {
        let argv: Vec<String> = vec![
            "test",
            "-destination",
            "platform=iOS Simulator,name=iPhone 16,OS=18.0",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let parsed = parse_xcodebuild_argv(&argv).unwrap();

        assert_eq!(
            parsed.flags.get("-destination"),
            Some(&Some(
                "platform=iOS Simulator,name=iPhone 16,OS=18.0".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_build_setting() {
        let argv: Vec<String> = vec!["build", "CODE_SIGN_IDENTITY=-", "-scheme", "MyApp"]
            .into_iter()
            .map(String::from)
            .collect();

        let parsed = parse_xcodebuild_argv(&argv).unwrap();

        assert!(parsed.flags.contains_key("CODE_SIGN_IDENTITY=-"));
    }

    #[test]
    fn test_multiple_actions_error() {
        let argv: Vec<String> = vec!["build", "test"]
            .into_iter()
            .map(String::from)
            .collect();

        let result = parse_xcodebuild_argv(&argv);
        assert!(result.is_err());
    }
}
