//! Validation engine for AgentMarket CLI.
//!
//! Orchestrates the validation workflow: retrieve deliverables, determine
//! pass/fail scores, and persist validation results. Actual I/O (chain,
//! IPFS) is delegated to callers -- this module contains pure business
//! logic and local filesystem persistence only.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::config::store::config_dir;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of validating a deliverable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationResult {
    /// The request ID that was validated.
    pub request_id: String,
    /// Whether the validation passed.
    pub passed: bool,
    /// Quality score (0-100).
    pub score: u8,
    /// Human-readable reason for the result.
    pub reason: String,
    /// Unix timestamp of validation.
    pub timestamp: u64,
}

/// Configuration for a validation handler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandlerConfig {
    /// Handler type: "manual", "external", or a built-in name.
    pub handler_type: String,
    /// Path to external handler executable (if handler_type == "external").
    pub executable: Option<String>,
    /// Timeout in seconds for handler execution.
    pub timeout_secs: u64,
    /// Additional environment variables to pass to the handler.
    pub env_vars: Vec<(String, String)>,
}

/// Input provided to a validation handler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandlerInput {
    /// The request ID.
    pub request_id: String,
    /// The task description from the original request.
    pub task_description: String,
    /// The decrypted deliverable content.
    pub deliverable: Vec<u8>,
    /// The seller's agent address.
    pub seller: String,
    /// The price in USDC (6 decimals).
    pub price_usdc: u64,
    /// The deadline as Unix timestamp.
    pub deadline: u64,
}

/// Output expected from a validation handler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandlerOutput {
    /// Quality score (0-100).
    pub score: u8,
    /// Human-readable reason.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

impl Default for HandlerConfig {
    fn default() -> Self {
        Self {
            handler_type: "manual".to_string(),
            executable: None,
            timeout_secs: 60,
            env_vars: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Passing threshold: score >= 60.
const PASSING_THRESHOLD: u8 = 60;

/// Name of the validations subdirectory inside the config directory.
const VALIDATIONS_DIR: &str = "validations";

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Determine pass/fail from a handler output.
///
/// Passing threshold: score >= 60.
pub fn is_passing(output: &HandlerOutput) -> bool {
    output.score >= PASSING_THRESHOLD
}

/// Create a [`ValidationResult`] from handler output.
///
/// The timestamp is set to the current system time.
pub fn create_result(request_id: &str, output: &HandlerOutput) -> ValidationResult {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    debug!(
        request_id = %request_id,
        score = output.score,
        passed = is_passing(output),
        "creating validation result"
    );

    ValidationResult {
        request_id: request_id.to_string(),
        passed: is_passing(output),
        score: output.score,
        reason: output.reason.clone(),
        timestamp: now,
    }
}

// ---------------------------------------------------------------------------
// Handler output parsing
// ---------------------------------------------------------------------------

/// Parse handler output from a JSON string (handler stdout).
///
/// The JSON must contain `score` (0-100) and `reason` (string) fields.
/// Returns an error if the JSON is malformed or the score exceeds 100.
pub fn parse_handler_output(json_str: &str) -> Result<HandlerOutput> {
    let output: HandlerOutput = serde_json::from_str(json_str).context(
        "failed to parse handler output — expected JSON with 'score' and 'reason' fields",
    )?;

    if output.score > 100 {
        anyhow::bail!("handler score must be 0-100, got {}", output.score);
    }

    debug!(
        score = output.score,
        reason = %output.reason,
        "parsed handler output"
    );

    Ok(output)
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Returns the path to the validations directory.
///
/// Creates the directory if it does not exist.
fn validations_dir() -> Result<PathBuf> {
    let dir = config_dir()?.join(VALIDATIONS_DIR);

    if !dir.exists() {
        debug!(path = %dir.display(), "creating validations directory");
        fs::create_dir_all(&dir).with_context(|| {
            format!("failed to create validations directory: {}", dir.display())
        })?;
    }

    Ok(dir)
}

/// Save a validation result to `~/.agentmarket/validations/{request_id}.json`.
pub fn save_result(result: &ValidationResult) -> Result<()> {
    let path = validations_dir()?.join(format!("{}.json", result.request_id));
    debug!(path = %path.display(), request_id = %result.request_id, "saving validation result");

    let json = serde_json::to_string_pretty(result)
        .context("failed to serialise validation result to JSON")?;

    fs::write(&path, json)
        .with_context(|| format!("failed to write validation result: {}", path.display()))?;

    debug!(path = %path.display(), "validation result saved");
    Ok(())
}

/// Load a validation result for the given request ID.
pub fn load_result(request_id: &str) -> Result<ValidationResult> {
    let path = validations_dir()?.join(format!("{}.json", request_id));
    debug!(path = %path.display(), "loading validation result");

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read validation result: {}", path.display()))?;

    let result: ValidationResult = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse validation result: {}", path.display()))?;

    debug!(request_id = %result.request_id, "validation result loaded");
    Ok(result)
}

/// Load all validation results from the validations directory.
pub fn load_all_results() -> Result<Vec<ValidationResult>> {
    let dir = validations_dir()?;
    debug!(path = %dir.display(), "loading all validation results");

    let mut results = Vec::new();

    let entries = fs::read_dir(&dir)
        .with_context(|| format!("failed to read validations directory: {}", dir.display()))?;

    for entry in entries {
        let entry = entry.context("failed to read directory entry")?;
        let path = entry.path();

        // Only process .json files.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read validation result: {}", path.display()))?;

        match serde_json::from_str::<ValidationResult>(&contents) {
            Ok(result) => results.push(result),
            Err(e) => {
                debug!(
                    path = %path.display(),
                    error = %e,
                    "skipping malformed validation result file"
                );
            }
        }
    }

    debug!(count = results.len(), "loaded validation results");
    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    /// Mutex to serialise tests that mutate environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: create a temporary directory and point `AGENTMARKET_HOME` at it
    /// for the duration of the closure. Restores (or removes) the env var
    /// afterwards. Acquires `ENV_LOCK` to prevent parallel env var mutation.
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let prev = env::var("AGENTMARKET_HOME").ok();

        env::set_var("AGENTMARKET_HOME", tmp.path());
        f();

        match prev {
            Some(v) => env::set_var("AGENTMARKET_HOME", v),
            None => env::remove_var("AGENTMARKET_HOME"),
        }
    }

    // -- is_passing -----------------------------------------------------------

    #[test]
    fn test_is_passing_threshold() {
        // Score 60 passes.
        let passing = HandlerOutput {
            score: 60,
            reason: "meets threshold".to_string(),
        };
        assert!(is_passing(&passing));

        // Score 59 fails.
        let failing = HandlerOutput {
            score: 59,
            reason: "below threshold".to_string(),
        };
        assert!(!is_passing(&failing));
    }

    #[test]
    fn test_is_passing_boundaries() {
        // Score 0 fails.
        let zero = HandlerOutput {
            score: 0,
            reason: "zero".to_string(),
        };
        assert!(!is_passing(&zero));

        // Score 100 passes.
        let hundred = HandlerOutput {
            score: 100,
            reason: "perfect".to_string(),
        };
        assert!(is_passing(&hundred));

        // Score 60 passes (exact threshold).
        let threshold = HandlerOutput {
            score: 60,
            reason: "threshold".to_string(),
        };
        assert!(is_passing(&threshold));
    }

    // -- create_result --------------------------------------------------------

    #[test]
    fn test_create_result_passing() {
        let output = HandlerOutput {
            score: 80,
            reason: "good quality".to_string(),
        };
        let result = create_result("req-001", &output);

        assert_eq!(result.request_id, "req-001");
        assert!(result.passed);
        assert_eq!(result.score, 80);
        assert_eq!(result.reason, "good quality");
        assert!(result.timestamp > 0);
    }

    #[test]
    fn test_create_result_failing() {
        let output = HandlerOutput {
            score: 30,
            reason: "poor quality".to_string(),
        };
        let result = create_result("req-002", &output);

        assert_eq!(result.request_id, "req-002");
        assert!(!result.passed);
        assert_eq!(result.score, 30);
        assert_eq!(result.reason, "poor quality");
        assert!(result.timestamp > 0);
    }

    // -- default handler config -----------------------------------------------

    #[test]
    fn test_default_handler_config() {
        let config = HandlerConfig::default();

        assert_eq!(config.handler_type, "manual");
        assert!(config.executable.is_none());
        assert_eq!(config.timeout_secs, 60);
        assert!(config.env_vars.is_empty());
    }

    // -- parse_handler_output -------------------------------------------------

    #[test]
    fn test_parse_handler_output_valid() {
        let json = r#"{"score": 85, "reason": "deliverable meets all requirements"}"#;
        let output = parse_handler_output(json).expect("parse should succeed");

        assert_eq!(output.score, 85);
        assert_eq!(output.reason, "deliverable meets all requirements");
    }

    #[test]
    fn test_parse_handler_output_invalid_json() {
        let bad_json = "not valid json at all";
        let result = parse_handler_output(bad_json);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("failed to parse handler output"),
            "error should mention parsing failure, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_parse_handler_output_score_too_high() {
        // Score > 100 should be rejected. u8 max is 255, so 150 is valid u8
        // but out of our 0-100 range.
        let json = r#"{"score": 150, "reason": "impossible score"}"#;
        let result = parse_handler_output(json);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("handler score must be 0-100"),
            "error should mention score range, got: {}",
            err_msg
        );
    }

    // -- persistence ----------------------------------------------------------

    #[test]
    fn test_save_load_result_roundtrip() {
        with_temp_home(|| {
            let result = ValidationResult {
                request_id: "req-roundtrip".to_string(),
                passed: true,
                score: 75,
                reason: "solid work".to_string(),
                timestamp: 1_700_000_000,
            };

            save_result(&result).expect("save should succeed");
            let loaded = load_result("req-roundtrip").expect("load should succeed");

            assert_eq!(loaded.request_id, "req-roundtrip");
            assert!(loaded.passed);
            assert_eq!(loaded.score, 75);
            assert_eq!(loaded.reason, "solid work");
            assert_eq!(loaded.timestamp, 1_700_000_000);
        });
    }

    #[test]
    fn test_load_all_results() {
        with_temp_home(|| {
            // Save multiple results.
            let r1 = ValidationResult {
                request_id: "req-all-1".to_string(),
                passed: true,
                score: 90,
                reason: "excellent".to_string(),
                timestamp: 1_700_000_001,
            };
            let r2 = ValidationResult {
                request_id: "req-all-2".to_string(),
                passed: false,
                score: 40,
                reason: "incomplete".to_string(),
                timestamp: 1_700_000_002,
            };
            let r3 = ValidationResult {
                request_id: "req-all-3".to_string(),
                passed: true,
                score: 65,
                reason: "acceptable".to_string(),
                timestamp: 1_700_000_003,
            };

            save_result(&r1).expect("save r1");
            save_result(&r2).expect("save r2");
            save_result(&r3).expect("save r3");

            let all = load_all_results().expect("load_all should succeed");
            assert_eq!(all.len(), 3);

            // Verify all request IDs are present (order may vary).
            let ids: Vec<&str> = all.iter().map(|r| r.request_id.as_str()).collect();
            assert!(ids.contains(&"req-all-1"));
            assert!(ids.contains(&"req-all-2"));
            assert!(ids.contains(&"req-all-3"));
        });
    }

    // -- serialization --------------------------------------------------------

    #[test]
    fn test_handler_input_serialization() {
        let input = HandlerInput {
            request_id: "req-ser-1".to_string(),
            task_description: "Build a widget".to_string(),
            deliverable: vec![0x48, 0x65, 0x6c, 0x6c, 0x6f], // "Hello"
            seller: "0xseller123".to_string(),
            price_usdc: 5_000_000,
            deadline: 1_700_100_000,
        };

        let json = serde_json::to_string(&input).expect("serialisation should succeed");
        let deserialized: HandlerInput =
            serde_json::from_str(&json).expect("deserialisation should succeed");

        assert_eq!(deserialized.request_id, "req-ser-1");
        assert_eq!(deserialized.task_description, "Build a widget");
        assert_eq!(deserialized.deliverable, vec![0x48, 0x65, 0x6c, 0x6c, 0x6f]);
        assert_eq!(deserialized.seller, "0xseller123");
        assert_eq!(deserialized.price_usdc, 5_000_000);
        assert_eq!(deserialized.deadline, 1_700_100_000);
    }

    #[test]
    fn test_validation_result_serialization() {
        let result = ValidationResult {
            request_id: "req-ser-2".to_string(),
            passed: true,
            score: 88,
            reason: "well done".to_string(),
            timestamp: 1_700_200_000,
        };

        let json = serde_json::to_string(&result).expect("serialisation should succeed");
        let deserialized: ValidationResult =
            serde_json::from_str(&json).expect("deserialisation should succeed");

        assert_eq!(deserialized.request_id, "req-ser-2");
        assert!(deserialized.passed);
        assert_eq!(deserialized.score, 88);
        assert_eq!(deserialized.reason, "well done");
        assert_eq!(deserialized.timestamp, 1_700_200_000);
    }
}
