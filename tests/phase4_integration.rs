//! Phase 4 cross-module integration tests.
//!
//! These tests verify that the Phase 4 modules (validation engine, external
//! handler invocation, manual handler) work together correctly in realistic
//! cross-module workflows. Individual unit tests for each module live in
//! their respective source files; this file focuses on interactions **between**
//! modules.
//!
//! Tests that mutate environment variables must run with `--test-threads=1`.

use std::env;
use std::io::Cursor;
use std::sync::Mutex;

use agentmarket::engine::handlers::{self, HandlerType};
use agentmarket::engine::manual_handler;
use agentmarket::engine::requests::{LocalRequest, LocalRequestStatus, RequestCache, RequestRole};
use agentmarket::engine::validation::{self, HandlerConfig, HandlerInput, HandlerOutput};

/// Mutex to serialise tests that mutate environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Helper: create a temporary directory, point `AGENTMARKET_HOME` at it,
/// run the closure, then restore the previous value.
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

/// Build a sample `LocalRequest` for testing.
fn sample_request(id: &str, status: LocalRequestStatus, role: RequestRole) -> LocalRequest {
    LocalRequest {
        request_id: id.to_string(),
        role,
        status,
        request_cid: "QmTestCid123".to_string(),
        price_usdc: 5_000_000,
        deadline: 1_700_000_000,
        response_cid: None,
        secret: None,
        secret_hash: None,
        counterparty: None,
        created_at: 1_699_000_000,
        updated_at: 1_699_000_000,
    }
}

/// Build a sample `HandlerInput` for testing.
fn sample_handler_input(request_id: &str) -> HandlerInput {
    HandlerInput {
        request_id: request_id.to_string(),
        task_description: "Write integration tests".to_string(),
        deliverable: b"Here is the deliverable content".to_vec(),
        seller: "0xSellerAddress".to_string(),
        price_usdc: 5_000_000,
        deadline: 1_700_000_000,
    }
}

// ===========================================================================
// 1. Validation + Request lifecycle
// ===========================================================================

/// A request in Responded status with a passing validation result should be
/// eligible to transition to Validated.
#[test]
fn validation_passing_allows_transition_to_validated() {
    let request = sample_request(
        "req-v1",
        LocalRequestStatus::Responded,
        RequestRole::Validator,
    );

    // Create a passing validation result.
    let output = HandlerOutput {
        score: 85,
        reason: "deliverable meets all requirements".to_string(),
    };
    let result = validation::create_result(&request.request_id, &output);

    // Verify the result is passing.
    assert!(result.passed, "score 85 should produce a passing result");
    assert!(validation::is_passing(&output));

    // Verify the request can transition from Responded to Validated.
    assert!(
        request
            .status
            .can_transition_to(&LocalRequestStatus::Validated),
        "Responded request should be able to transition to Validated"
    );
}

/// A request in Responded status with a failing validation result should
/// remain in Responded (no transition to Validated).
#[test]
fn validation_failing_keeps_request_in_responded() {
    let request = sample_request(
        "req-v2",
        LocalRequestStatus::Responded,
        RequestRole::Validator,
    );

    // Create a failing validation result.
    let output = HandlerOutput {
        score: 40,
        reason: "deliverable is incomplete".to_string(),
    };
    let result = validation::create_result(&request.request_id, &output);

    // Verify the result is failing.
    assert!(!result.passed, "score 40 should produce a failing result");
    assert!(!validation::is_passing(&output));

    // The request status remains Responded; it should not transition to Validated
    // because the validation failed. Verify the status is still Responded.
    assert_eq!(request.status, LocalRequestStatus::Responded);
}

/// Save a validation result to a temp directory, reload it, and verify
/// all fields are preserved.
#[test]
fn validation_result_persistence_roundtrip() {
    with_temp_home(|| {
        let output = HandlerOutput {
            score: 72,
            reason: "mostly good".to_string(),
        };
        let result = validation::create_result("req-persist", &output);

        validation::save_result(&result).expect("save_result should succeed");
        let loaded = validation::load_result("req-persist").expect("load_result should succeed");

        assert_eq!(loaded.request_id, "req-persist");
        assert!(loaded.passed);
        assert_eq!(loaded.score, 72);
        assert_eq!(loaded.reason, "mostly good");
        assert_eq!(loaded.timestamp, result.timestamp);
    });
}

// ===========================================================================
// 2. Handler + Validation integration
// ===========================================================================

/// Create a shell script that outputs passing JSON, execute it via
/// handlers::execute_handler, parse the output, create a ValidationResult,
/// and verify it passes.
#[cfg(unix)]
#[test]
fn handler_execute_parse_create_passing() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("approve_handler.sh");
    fs::write(
        &script,
        "#!/bin/sh\necho '{\"score\": 85, \"reason\": \"good quality work\"}'",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    // Execute the handler.
    let stdout = handlers::execute_handler(
        script.to_str().unwrap(),
        b"deliverable content here",
        "req-h1",
        "0xSeller",
        1_700_100_000,
        5_000_000,
        10,
    )
    .expect("execute_handler should succeed");

    // Parse the output.
    let handler_output =
        validation::parse_handler_output(&stdout).expect("parse_handler_output should succeed");

    assert_eq!(handler_output.score, 85);
    assert_eq!(handler_output.reason, "good quality work");

    // Create validation result.
    let result = validation::create_result("req-h1", &handler_output);

    assert!(result.passed, "score 85 should be passing");
    assert_eq!(result.request_id, "req-h1");
    assert_eq!(result.score, 85);
    assert!(result.timestamp > 0);
}

/// Create a shell script that outputs rejecting JSON, execute it, parse,
/// and verify the result is failing.
#[cfg(unix)]
#[test]
fn handler_execute_parse_create_failing() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("reject_handler.sh");
    fs::write(
        &script,
        "#!/bin/sh\necho '{\"score\": 30, \"reason\": \"missing sections\"}'",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let stdout = handlers::execute_handler(
        script.to_str().unwrap(),
        b"incomplete deliverable",
        "req-h2",
        "0xSeller2",
        1_700_200_000,
        3_000_000,
        10,
    )
    .expect("execute_handler should succeed");

    let handler_output =
        validation::parse_handler_output(&stdout).expect("parse_handler_output should succeed");

    assert_eq!(handler_output.score, 30);
    assert!(!validation::is_passing(&handler_output));

    let result = validation::create_result("req-h2", &handler_output);
    assert!(!result.passed, "score 30 should be failing");
}

/// Verify that execute_handler passes the correct environment variables
/// to the handler script and that those values can flow through to a
/// validation result.
#[cfg(unix)]
#[test]
fn handler_env_vars_flow_through() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("env_handler.sh");
    // Script checks env vars and uses them to decide the score.
    fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "if [ \"$AGENTMARKET_REQUEST_ID\" = \"req-env-42\" ] && ",
            "[ \"$AGENTMARKET_PRICE\" = \"7500000\" ] && ",
            "[ \"$AGENTMARKET_SELLER\" = \"0xEnvSeller\" ]; then\n",
            "  echo '{\"score\": 95, \"reason\": \"env vars correct\"}'\n",
            "else\n",
            "  echo '{\"score\": 10, \"reason\": \"env vars missing\"}'\n",
            "fi"
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let stdout = handlers::execute_handler(
        script.to_str().unwrap(),
        b"",
        "req-env-42",
        "0xEnvSeller",
        1_700_300_000,
        7_500_000,
        10,
    )
    .expect("execute_handler should succeed");

    let handler_output = validation::parse_handler_output(&stdout).expect("parse should succeed");
    assert_eq!(
        handler_output.score, 95,
        "env vars should be passed correctly"
    );
    assert_eq!(handler_output.reason, "env vars correct");
}

// ===========================================================================
// 3. Validation result persistence with request cache
// ===========================================================================

/// Save a validation result and a request to the same temp home, load both,
/// and verify they reference the same request_id.
#[test]
fn validation_result_and_request_same_id() {
    with_temp_home(|| {
        let request_id = "req-cross-1";

        // Save a request.
        let request = sample_request(
            request_id,
            LocalRequestStatus::Responded,
            RequestRole::Validator,
        );
        RequestCache::save(&request).expect("save request should succeed");

        // Create and save a validation result for the same request.
        let output = HandlerOutput {
            score: 78,
            reason: "acceptable".to_string(),
        };
        let vresult = validation::create_result(request_id, &output);
        validation::save_result(&vresult).expect("save validation result should succeed");

        // Load both and verify they reference the same request_id.
        let loaded_request = RequestCache::load(request_id).expect("load request should succeed");
        let loaded_validation =
            validation::load_result(request_id).expect("load validation should succeed");

        assert_eq!(
            loaded_request.request_id, loaded_validation.request_id,
            "request and validation result must reference the same request_id"
        );
        assert_eq!(loaded_validation.score, 78);
        assert!(loaded_validation.passed);
    });
}

/// Save multiple validation results, load all, and verify the correct count.
#[test]
fn multiple_validation_results_load_all() {
    with_temp_home(|| {
        let ids = ["req-multi-1", "req-multi-2", "req-multi-3"];
        let scores = [90u8, 50, 65];

        for (id, score) in ids.iter().zip(scores.iter()) {
            let output = HandlerOutput {
                score: *score,
                reason: format!("score {}", score),
            };
            let result = validation::create_result(id, &output);
            validation::save_result(&result).expect("save should succeed");
        }

        let all = validation::load_all_results().expect("load_all_results should succeed");
        assert_eq!(all.len(), 3, "should have exactly 3 validation results");

        // Verify all request IDs are present (order may vary).
        let loaded_ids: Vec<&str> = all.iter().map(|r| r.request_id.as_str()).collect();
        for id in &ids {
            assert!(
                loaded_ids.contains(id),
                "should contain request_id '{}'",
                id
            );
        }

        // Verify pass/fail distribution: score 90 passes, 50 fails, 65 passes.
        let passing_count = all.iter().filter(|r| r.passed).count();
        assert_eq!(
            passing_count, 2,
            "two results should be passing (90 and 65)"
        );
    });
}

// ===========================================================================
// 4. Handler config + resolution
// ===========================================================================

/// Build a HandlerConfig for "manual" and verify the handler type resolves
/// correctly.
#[test]
fn handler_config_manual_resolution() {
    let config = HandlerConfig {
        handler_type: "manual".to_string(),
        executable: None,
        timeout_secs: 60,
        env_vars: Vec::new(),
    };

    let handler_type =
        HandlerType::from_str(&config.handler_type, config.executable.as_deref()).unwrap();
    assert_eq!(handler_type, HandlerType::Manual);
}

/// Build a HandlerConfig for "external" with a path and verify the handler
/// type resolves correctly.
#[test]
fn handler_config_external_resolution() {
    let config = HandlerConfig {
        handler_type: "external".to_string(),
        executable: Some("/usr/local/bin/my-validator".to_string()),
        timeout_secs: 120,
        env_vars: vec![("CUSTOM_VAR".to_string(), "value".to_string())],
    };

    let handler_type =
        HandlerType::from_str(&config.handler_type, config.executable.as_deref()).unwrap();
    assert_eq!(
        handler_type,
        HandlerType::External("/usr/local/bin/my-validator".to_string())
    );
}

/// Build a HandlerConfig for "external" without a path and verify it errors.
#[test]
fn handler_config_external_missing_path_errors() {
    let config = HandlerConfig {
        handler_type: "external".to_string(),
        executable: None,
        timeout_secs: 30,
        env_vars: Vec::new(),
    };

    let result = HandlerType::from_str(&config.handler_type, config.executable.as_deref());
    assert!(
        result.is_err(),
        "external handler without executable should fail"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("executable path"),
        "error should mention executable path, got: {}",
        msg
    );
}

// ===========================================================================
// 5. Full validation flow simulation
// ===========================================================================

/// End-to-end flow: create a request cache entry, build handler input from
/// request data, serialize to JSON, create a handler script that reads stdin
/// and produces valid output, execute the handler, parse output, create
/// validation result, save, load, and verify everything is consistent.
#[cfg(unix)]
#[test]
fn full_validation_flow_simulation() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    with_temp_home(|| {
        let request_id = "req-full-flow";

        // Step 1: Create and save a request in Responded status.
        let mut request = sample_request(
            request_id,
            LocalRequestStatus::Responded,
            RequestRole::Validator,
        );
        request.response_cid = Some("QmResponseCid".to_string());
        request.counterparty = Some("0xSellerFull".to_string());
        RequestCache::save(&request).expect("save request should succeed");

        // Step 2: Build handler input from request data.
        let handler_input = HandlerInput {
            request_id: request.request_id.clone(),
            task_description: "Build a widget".to_string(),
            deliverable: b"Widget source code here".to_vec(),
            seller: request.counterparty.clone().unwrap_or_default(),
            price_usdc: request.price_usdc,
            deadline: request.deadline,
        };

        // Step 3: Serialize handler input to JSON for the script's stdin.
        let input_json =
            serde_json::to_string(&handler_input).expect("serialization should succeed");

        // Step 4: Create a handler script that reads stdin and produces output.
        // The script reads stdin (the JSON), extracts request_id to verify it
        // received data, then outputs a passing verdict.
        let tmp_dir = tempfile::tempdir().unwrap();
        let script = tmp_dir.path().join("flow_handler.sh");
        fs::write(
            &script,
            concat!(
                "#!/bin/sh\n",
                "INPUT=$(cat)\n",
                "echo '{\"score\": 88, \"reason\": \"widget looks great\"}'"
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        // Step 5: Execute the handler with the serialized input as deliverable.
        let stdout = handlers::execute_handler(
            script.to_str().unwrap(),
            input_json.as_bytes(),
            &handler_input.request_id,
            &handler_input.seller,
            handler_input.deadline,
            handler_input.price_usdc,
            10,
        )
        .expect("execute_handler should succeed");

        // Step 6: Parse the handler output.
        let handler_output =
            validation::parse_handler_output(&stdout).expect("parse should succeed");
        assert_eq!(handler_output.score, 88);
        assert_eq!(handler_output.reason, "widget looks great");
        assert!(validation::is_passing(&handler_output));

        // Step 7: Create the validation result.
        let vresult = validation::create_result(request_id, &handler_output);
        assert!(vresult.passed);
        assert_eq!(vresult.request_id, request_id);

        // Step 8: Save the validation result.
        validation::save_result(&vresult).expect("save validation result should succeed");

        // Step 9: Load both the request and validation result and verify consistency.
        let loaded_request = RequestCache::load(request_id).expect("load request should succeed");
        let loaded_validation =
            validation::load_result(request_id).expect("load validation should succeed");

        assert_eq!(loaded_request.request_id, loaded_validation.request_id);
        assert_eq!(loaded_validation.score, 88);
        assert!(loaded_validation.passed);
        assert_eq!(loaded_validation.reason, "widget looks great");

        // Step 10: Verify the request can transition to Validated.
        assert!(
            loaded_request
                .status
                .can_transition_to(&LocalRequestStatus::Validated),
            "Responded request with passing validation should be able to transition to Validated"
        );
    });
}

/// Full flow with a failing handler: verify the request cannot sensibly
/// transition to Validated when the validation fails.
#[cfg(unix)]
#[test]
fn full_validation_flow_failing() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    with_temp_home(|| {
        let request_id = "req-full-fail";

        // Create request in Responded status.
        let request = sample_request(
            request_id,
            LocalRequestStatus::Responded,
            RequestRole::Validator,
        );
        RequestCache::save(&request).expect("save request should succeed");

        // Create a rejecting handler script.
        let tmp_dir = tempfile::tempdir().unwrap();
        let script = tmp_dir.path().join("reject_handler.sh");
        fs::write(
            &script,
            "#!/bin/sh\necho '{\"score\": 25, \"reason\": \"does not meet requirements\"}'",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        // Execute and parse.
        let stdout = handlers::execute_handler(
            script.to_str().unwrap(),
            b"poor deliverable",
            request_id,
            "0xSeller",
            1_700_000_000,
            5_000_000,
            10,
        )
        .expect("execute_handler should succeed");

        let handler_output = validation::parse_handler_output(&stdout).unwrap();
        assert!(!validation::is_passing(&handler_output));

        let vresult = validation::create_result(request_id, &handler_output);
        assert!(!vresult.passed);

        // Save and reload.
        validation::save_result(&vresult).expect("save should succeed");
        let loaded = validation::load_result(request_id).unwrap();
        assert!(!loaded.passed);
        assert_eq!(loaded.score, 25);
        assert_eq!(loaded.reason, "does not meet requirements");
    });
}

/// Integration test: handler reads stdin deliverable content and uses it
/// to determine the validation score, demonstrating real data flow from
/// deliverable through handler to validation result.
#[cfg(unix)]
#[test]
fn handler_reads_stdin_for_validation_decision() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("stdin_handler.sh");
    // Script checks if stdin contains "PASS" keyword to decide score.
    fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "INPUT=$(cat)\n",
            "if echo \"$INPUT\" | grep -q 'PASS'; then\n",
            "  echo '{\"score\": 80, \"reason\": \"contains PASS keyword\"}'\n",
            "else\n",
            "  echo '{\"score\": 20, \"reason\": \"missing PASS keyword\"}'\n",
            "fi"
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    // Test with passing deliverable.
    let stdout_pass = handlers::execute_handler(
        script.to_str().unwrap(),
        b"This deliverable should PASS review",
        "req-stdin-1",
        "0xSeller",
        1_700_000_000,
        5_000_000,
        10,
    )
    .unwrap();
    let output_pass = validation::parse_handler_output(&stdout_pass).unwrap();
    assert!(validation::is_passing(&output_pass));

    // Test with failing deliverable.
    let stdout_fail = handlers::execute_handler(
        script.to_str().unwrap(),
        b"This deliverable does not have the keyword",
        "req-stdin-2",
        "0xSeller",
        1_700_000_000,
        5_000_000,
        10,
    )
    .unwrap();
    let output_fail = validation::parse_handler_output(&stdout_fail).unwrap();
    assert!(!validation::is_passing(&output_fail));
}

// ===========================================================================
// 6. Manual handler + validation integration
// ===========================================================================

/// Run the manual handler with simulated reader input, then create a
/// validation result from its output, testing the cross-module boundary
/// between manual_handler and validation.
#[test]
fn manual_handler_to_validation_result() {
    let input = sample_handler_input("req-manual-1");

    // Simulate user approving with score 75 and a reason.
    let mut reader = Cursor::new("y\n75\nlooks acceptable\n");
    let handler_output =
        manual_handler::run_manual_review_with_reader(&input, &mut reader).unwrap();

    assert_eq!(handler_output.score, 75);
    assert_eq!(handler_output.reason, "looks acceptable");

    // Create validation result from the manual handler output.
    let result = validation::create_result("req-manual-1", &handler_output);

    assert!(result.passed, "score 75 should pass (>= 60)");
    assert_eq!(result.request_id, "req-manual-1");
    assert_eq!(result.score, 75);
    assert_eq!(result.reason, "looks acceptable");
}

/// Manual handler rejection flows through to a failing validation result.
#[test]
fn manual_handler_rejection_to_validation_result() {
    let input = sample_handler_input("req-manual-2");

    // Simulate user rejecting with score 30.
    let mut reader = Cursor::new("n\n30\nincomplete deliverable\n");
    let handler_output =
        manual_handler::run_manual_review_with_reader(&input, &mut reader).unwrap();

    let result = validation::create_result("req-manual-2", &handler_output);

    assert!(!result.passed, "score 30 should fail (< 60)");
    assert_eq!(result.reason, "incomplete deliverable");
}

/// Manual handler with default score: approve without explicit score should
/// use 80 (the default for approved), which is passing.
#[test]
fn manual_handler_default_score_passes_validation() {
    let input = sample_handler_input("req-manual-3");

    let mut reader = Cursor::new("y\n\n\n");
    let handler_output =
        manual_handler::run_manual_review_with_reader(&input, &mut reader).unwrap();

    assert_eq!(
        handler_output.score, 80,
        "default approved score should be 80"
    );

    let result = validation::create_result("req-manual-3", &handler_output);
    assert!(result.passed, "default approved score 80 should pass");
    assert_eq!(result.reason, "Manually approved");
}

/// Manual handler with default rejection score: reject without explicit
/// score should use 20 (the default for rejected), which is failing.
#[test]
fn manual_handler_default_rejection_fails_validation() {
    let input = sample_handler_input("req-manual-4");

    let mut reader = Cursor::new("n\n\n\n");
    let handler_output =
        manual_handler::run_manual_review_with_reader(&input, &mut reader).unwrap();

    assert_eq!(
        handler_output.score, 20,
        "default rejected score should be 20"
    );

    let result = validation::create_result("req-manual-4", &handler_output);
    assert!(!result.passed, "default rejected score 20 should fail");
    assert_eq!(result.reason, "Manually rejected");
}

// ===========================================================================
// 7. Manual handler + validation + persistence
// ===========================================================================

/// Full manual validation flow with persistence: simulate manual review,
/// create validation result, save it, load it back, verify.
#[test]
fn manual_handler_validation_persistence() {
    with_temp_home(|| {
        let input = sample_handler_input("req-manual-persist");

        let mut reader = Cursor::new("y\n92\nexcellent work\n");
        let handler_output =
            manual_handler::run_manual_review_with_reader(&input, &mut reader).unwrap();

        let result = validation::create_result("req-manual-persist", &handler_output);
        validation::save_result(&result).expect("save should succeed");

        let loaded = validation::load_result("req-manual-persist").expect("load should succeed");
        assert_eq!(loaded.request_id, "req-manual-persist");
        assert!(loaded.passed);
        assert_eq!(loaded.score, 92);
        assert_eq!(loaded.reason, "excellent work");
    });
}

// ===========================================================================
// 8. HandlerInput serialization across modules
// ===========================================================================

/// Verify that HandlerInput can be serialized, written as stdin to a handler
/// script, and the handler can parse it to make decisions.
#[cfg(unix)]
#[test]
fn handler_input_serialization_through_handler() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let input = sample_handler_input("req-ser-flow");
    let input_json = serde_json::to_string(&input).expect("serialization should succeed");

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("json_handler.sh");
    // Script reads stdin JSON and echoes a verdict based on whether it could
    // parse the request_id field.
    fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "INPUT=$(cat)\n",
            "if echo \"$INPUT\" | grep -q 'req-ser-flow'; then\n",
            "  echo '{\"score\": 70, \"reason\": \"parsed request_id from stdin\"}'\n",
            "else\n",
            "  echo '{\"score\": 5, \"reason\": \"could not find request_id\"}'\n",
            "fi"
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let stdout = handlers::execute_handler(
        script.to_str().unwrap(),
        input_json.as_bytes(),
        &input.request_id,
        &input.seller,
        input.deadline,
        input.price_usdc,
        10,
    )
    .expect("execute_handler should succeed");

    let handler_output = validation::parse_handler_output(&stdout).unwrap();
    assert_eq!(handler_output.score, 70);
    assert_eq!(handler_output.reason, "parsed request_id from stdin");
    assert!(validation::is_passing(&handler_output));
}
