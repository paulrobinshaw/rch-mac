//! xcodebuild argv parser.
//!
//! Parses xcodebuild command-line arguments into a structured representation.

/// Known xcodebuild actions (bare words).
const KNOWN_ACTIONS: &[&str] = &[
    "build",
    "build-for-testing",
    "analyze",
    "archive",
    "test",
    "test-without-building",
    "installsrc",
    "install",
    "clean",
    "docbuild",
    "exportArchive",
    "exportNotarizedApp",
    "exportLocalizations",
    "importLocalizations",
];

/// Known flags that take a value.
const FLAGS_WITH_VALUES: &[&str] = &[
    "-workspace",
    "-project",
    "-target",
    "-alltargets",
    "-scheme",
    "-destination",
    "-configuration",
    "-arch",
    "-sdk",
    "-toolchain",
    "-jobs",
    "-parallelizeTargets",
    "-showBuildTimingSummary",
    "-resultBundlePath",
    "-derivedDataPath",
    "-archivePath",
    "-exportPath",
    "-exportOptionsPlist",
    "-clonedSourcePackagesDirPath",
    "-xctestrun",
    "-testPlan",
    "-only-testing",
    "-skip-testing",
    "-maximum-concurrent-test-device-destinations",
    "-maximum-concurrent-test-simulator-destinations",
    "-test-iterations",
    "-retry-tests-on-failure",
    "-test-repetition-relaunch-enabled",
    "-resultStreamPath",
    "-IDEPackageSupportUseBuiltinSCM",
    "-skipPackagePluginValidation",
    "-skipMacroValidation",
    "-xcconfig",
    "-xctarget",
    "-xcroot",
    "-buildstyle",
    "-installpath",
    "-objroot",
    "-symroot",
    "-dstroot",
    "-exportLanguage",
    "-localizationPath",
    "-localization",
];

/// Known boolean flags (no value).
const BOOLEAN_FLAGS: &[&str] = &[
    "-quiet",
    "-verbose",
    "-hideShellScriptEnvironment",
    "-showsdks",
    "-showdestinations",
    "-showBuildSettings",
    "-showBuildSettingsForIndex",
    "-list",
    "-find-executable",
    "-find-library",
    "-version",
    "-usage",
    "-license",
    "-checkFirstLaunchStatus",
    "-runFirstLaunch",
    "-downloadPlatform",
    "-downloadAllPlatforms",
    "-exportNotarizedApp",
    "-enableCodeCoverage",
    "-disableCodeCoverage",
    "-enableAddressSanitizer",
    "-enableThreadSanitizer",
    "-enableUndefinedBehaviorSanitizer",
    "-testLanguage",
    "-testRegion",
    "-parallel-testing-enabled",
    "-allowProvisioningUpdates",
    "-allowProvisioningDeviceRegistration",
    "-showTestPlans",
    "-json",
    "-dry-run",
    "-n",
];

/// Parsed xcodebuild arguments.
#[derive(Debug, Clone, Default)]
pub struct ParsedArgv {
    /// The action (e.g., "build", "test"). None if not specified.
    pub action: Option<String>,

    /// Flags and their values. Boolean flags have None as value.
    pub flags: Vec<(String, Option<String>)>,

    /// Build settings (KEY=VALUE pairs).
    pub build_settings: Vec<(String, String)>,
}

/// Parse xcodebuild argv into a structured representation.
///
/// # Arguments
/// * `argv` - The arguments passed to xcodebuild (not including "xcodebuild" itself).
///
/// # Returns
/// A parsed representation of the arguments, or an error message.
pub fn parse_argv(argv: &[String]) -> Result<ParsedArgv, String> {
    let mut parsed = ParsedArgv::default();
    let mut i = 0;

    while i < argv.len() {
        let arg = &argv[i];

        // Check for action (bare word that matches known actions)
        if !arg.starts_with('-') && !arg.contains('=') {
            if KNOWN_ACTIONS.contains(&arg.as_str()) {
                if parsed.action.is_some() {
                    return Err(format!(
                        "multiple actions specified: {} and {}",
                        parsed.action.as_ref().unwrap(),
                        arg
                    ));
                }
                parsed.action = Some(arg.clone());
                i += 1;
                continue;
            }
            // Unknown bare word - might be build setting without =
            // For safety, treat as unknown and let classifier reject
            return Err(format!("unknown bare word: {}", arg));
        }

        // Check for build setting (KEY=VALUE)
        if !arg.starts_with('-') && arg.contains('=') {
            let parts: Vec<&str> = arg.splitn(2, '=').collect();
            if parts.len() == 2 {
                parsed
                    .build_settings
                    .push((parts[0].to_string(), parts[1].to_string()));
                i += 1;
                continue;
            }
        }

        // Check for flag
        if arg.starts_with('-') {
            // Handle flags with = inline (e.g., -destination=platform=iOS)
            if let Some(eq_pos) = arg.find('=') {
                let (flag, value) = arg.split_at(eq_pos);
                let value = &value[1..]; // Skip the '='
                parsed.flags.push((flag.to_string(), Some(value.to_string())));
                i += 1;
                continue;
            }

            // Check if this is a boolean flag
            if BOOLEAN_FLAGS.contains(&arg.as_str()) {
                parsed.flags.push((arg.clone(), None));
                i += 1;
                continue;
            }

            // Check if this flag takes a value
            if FLAGS_WITH_VALUES.contains(&arg.as_str()) {
                if i + 1 >= argv.len() {
                    return Err(format!("flag {} requires a value", arg));
                }
                let value = &argv[i + 1];
                // Value shouldn't start with - (likely another flag)
                if value.starts_with('-') && !value.starts_with("-") {
                    // Allow values like "-destination" that start with -
                    // but only if it's part of the value
                    if FLAGS_WITH_VALUES.contains(&value.as_str())
                        || BOOLEAN_FLAGS.contains(&value.as_str())
                    {
                        return Err(format!("flag {} requires a value, got flag {}", arg, value));
                    }
                }
                parsed.flags.push((arg.clone(), Some(value.clone())));
                i += 2;
                continue;
            }

            // Unknown flag - store it and let classifier decide
            // Check if next arg could be a value
            if i + 1 < argv.len() {
                let next = &argv[i + 1];
                if !next.starts_with('-') && !KNOWN_ACTIONS.contains(&next.as_str()) {
                    // Assume it takes a value
                    parsed.flags.push((arg.clone(), Some(next.clone())));
                    i += 2;
                    continue;
                }
            }
            // Treat as boolean
            parsed.flags.push((arg.clone(), None));
            i += 1;
            continue;
        }

        i += 1;
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_argv(s: &str) -> Vec<String> {
        // Simple split - doesn't handle quoted strings
        s.split_whitespace().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_parse_simple_build() {
        let argv = to_argv("build -workspace Foo.xcworkspace -scheme Bar");
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(parsed.action, Some("build".to_string()));
        assert_eq!(parsed.flags.len(), 2);
        assert_eq!(
            parsed.flags[0],
            ("-workspace".to_string(), Some("Foo.xcworkspace".to_string()))
        );
        assert_eq!(
            parsed.flags[1],
            ("-scheme".to_string(), Some("Bar".to_string()))
        );
    }

    #[test]
    fn test_parse_no_action() {
        let argv = to_argv("-workspace Foo.xcworkspace -scheme Bar");
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(parsed.action, None);
        assert_eq!(parsed.flags.len(), 2);
    }

    #[test]
    fn test_parse_test_action() {
        let argv = to_argv("test -scheme MyTests");
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(parsed.action, Some("test".to_string()));
    }

    #[test]
    fn test_parse_boolean_flag() {
        let argv = to_argv("-quiet build -scheme Bar");
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(parsed.action, Some("build".to_string()));
        assert!(parsed
            .flags
            .iter()
            .any(|(f, v)| f == "-quiet" && v.is_none()));
    }

    #[test]
    fn test_parse_build_setting() {
        let argv = to_argv("build CODE_SIGNING_ALLOWED=NO");
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(parsed.build_settings.len(), 1);
        assert_eq!(
            parsed.build_settings[0],
            ("CODE_SIGNING_ALLOWED".to_string(), "NO".to_string())
        );
    }

    #[test]
    fn test_parse_action_anywhere() {
        let argv = to_argv("-workspace Foo.xcworkspace build -scheme Bar");
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(parsed.action, Some("build".to_string()));
    }

    #[test]
    fn test_parse_multiple_actions_error() {
        let argv = to_argv("build test");
        let result = parse_argv(&argv);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("multiple actions"));
    }

    #[test]
    fn test_parse_destination_with_spaces() {
        // Note: this test uses the simple split which doesn't handle quotes
        // In real usage, the argv would be properly split by the shell
        let argv = vec![
            "build".to_string(),
            "-destination".to_string(),
            "platform=iOS Simulator,name=iPhone 16".to_string(),
        ];
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(
            parsed.flags[0],
            (
                "-destination".to_string(),
                Some("platform=iOS Simulator,name=iPhone 16".to_string())
            )
        );
    }

    #[test]
    fn test_parse_flag_with_equals() {
        let argv = vec![
            "-destination=platform=iOS".to_string(),
            "build".to_string(),
        ];
        let parsed = parse_argv(&argv).unwrap();
        assert_eq!(
            parsed.flags[0],
            ("-destination".to_string(), Some("platform=iOS".to_string()))
        );
    }
}
