//! External validation handler invocation for AgentMarket CLI.
//!
//! Validation handlers are external processes that receive a deliverable on
//! stdin and return a JSON verdict on stdout. This module manages process
//! lifecycle, environment setup, timeouts, and I/O.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::debug;

// ---------------------------------------------------------------------------
// Handler types
// ---------------------------------------------------------------------------

/// Supported handler types for validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandlerType {
    /// Manual approval via terminal prompt.
    Manual,
    /// External process handler with the path to an executable.
    External(String),
}

impl HandlerType {
    /// Parse a handler type from a string descriptor.
    ///
    /// * `"manual"` -- returns [`HandlerType::Manual`].
    /// * `"external"` -- returns [`HandlerType::External`] with the given
    ///   `executable` path. Fails if `executable` is `None`.
    pub fn from_str(s: &str, executable: Option<&str>) -> Result<Self> {
        match s {
            "manual" => Ok(HandlerType::Manual),
            "external" => {
                let path = executable.ok_or_else(|| {
                    anyhow::anyhow!("external handler requires an executable path")
                })?;
                Ok(HandlerType::External(path.to_string()))
            }
            other => anyhow::bail!("unknown handler type: {}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// Timeout helper
// ---------------------------------------------------------------------------

/// Wait for a child process to complete, enforcing a timeout.
///
/// Spawns a background thread that calls `wait_with_output()` on the child.
/// If the child does not finish within `timeout`, returns an error.
///
/// Note: on timeout the child process may still be running. A production
/// implementation would kill the process; for now the timeout error is
/// sufficient.
fn wait_with_timeout(
    child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result.context("handler process failed"),
        Err(_) => {
            anyhow::bail!("handler timed out after {} seconds", timeout.as_secs());
        }
    }
}

// ---------------------------------------------------------------------------
// Handler execution
// ---------------------------------------------------------------------------

/// Execute an external handler process.
///
/// The handler receives the deliverable content on stdin and is expected
/// to write JSON output to stdout: `{"score": N, "reason": "..."}`.
///
/// Environment variables set for the handler:
/// - `AGENTMARKET_REQUEST_ID`
/// - `AGENTMARKET_TASK_TYPE` (empty for now)
/// - `AGENTMARKET_SELLER`
/// - `AGENTMARKET_DEADLINE`
/// - `AGENTMARKET_PRICE` (USDC amount as string)
///
/// Returns the raw stdout output as a String.
pub fn execute_handler(
    executable: &str,
    deliverable: &[u8],
    request_id: &str,
    seller: &str,
    deadline: u64,
    price_usdc: u64,
    timeout_secs: u64,
) -> Result<String> {
    debug!(
        executable = %executable,
        request_id = %request_id,
        seller = %seller,
        deadline = %deadline,
        price_usdc = %price_usdc,
        timeout_secs = %timeout_secs,
        "executing external handler"
    );

    let mut child = Command::new(executable)
        .env("AGENTMARKET_REQUEST_ID", request_id)
        .env("AGENTMARKET_TASK_TYPE", "")
        .env("AGENTMARKET_SELLER", seller)
        .env("AGENTMARKET_DEADLINE", deadline.to_string())
        .env("AGENTMARKET_PRICE", price_usdc.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn handler: {}", executable))?;

    // Write deliverable to stdin, then close it so the handler sees EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(deliverable)
            .context("failed to write deliverable to handler stdin")?;
        // stdin is dropped here, closing the pipe.
    }

    let timeout = Duration::from_secs(timeout_secs);
    let output = wait_with_timeout(child, timeout)?;

    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        anyhow::bail!("handler exited with code {}: {}", code, stderr_text.trim());
    }

    let stdout_text =
        String::from_utf8(output.stdout).context("handler stdout contained invalid UTF-8")?;

    debug!(
        output_len = stdout_text.len(),
        "handler finished successfully"
    );

    Ok(stdout_text)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- HandlerType parsing -------------------------------------------------

    #[test]
    fn test_handler_type_manual() {
        let ht = HandlerType::from_str("manual", None).unwrap();
        assert_eq!(ht, HandlerType::Manual);
    }

    #[test]
    fn test_handler_type_manual_ignores_executable() {
        // Even if an executable is provided, "manual" should ignore it.
        let ht = HandlerType::from_str("manual", Some("/usr/bin/foo")).unwrap();
        assert_eq!(ht, HandlerType::Manual);
    }

    #[test]
    fn test_handler_type_external() {
        let ht = HandlerType::from_str("external", Some("/usr/bin/my-handler")).unwrap();
        assert_eq!(ht, HandlerType::External("/usr/bin/my-handler".to_string()));
    }

    #[test]
    fn test_handler_type_external_no_path_fails() {
        let result = HandlerType::from_str("external", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("executable path"),
            "error should mention executable path, got: {}",
            msg
        );
    }

    #[test]
    fn test_handler_type_unknown_fails() {
        let result = HandlerType::from_str("automagic", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown handler type"),
            "error should mention unknown handler type, got: {}",
            msg
        );
    }

    // -- execute_handler -----------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_execute_handler_echo() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("handler.sh");
        fs::write(
            &script,
            "#!/bin/sh\necho '{\"score\": 85, \"reason\": \"looks good\"}'",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let result = execute_handler(
            script.to_str().unwrap(),
            b"test deliverable content",
            "req-1",
            "seller-addr",
            9999999,
            5000000,
            10,
        )
        .unwrap();

        assert!(result.contains("score"), "output should contain 'score'");
        assert!(result.contains("85"), "output should contain score value");
        assert!(
            result.contains("looks good"),
            "output should contain reason"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_execute_handler_receives_stdin() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("handler.sh");
        // Script that reads stdin and echoes it back wrapped in JSON.
        fs::write(
            &script,
            "#!/bin/sh\nINPUT=$(cat)\necho \"{\\\"received\\\": \\\"$INPUT\\\"}\"",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let result = execute_handler(
            script.to_str().unwrap(),
            b"hello from test",
            "req-2",
            "seller-2",
            1000000,
            100,
            10,
        )
        .unwrap();

        assert!(
            result.contains("hello from test"),
            "handler should receive stdin content, got: {}",
            result
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_execute_handler_receives_env_vars() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("handler.sh");
        // Script that outputs the environment variables as JSON.
        fs::write(
            &script,
            concat!(
                "#!/bin/sh\n",
                "echo \"{",
                "\\\"request_id\\\": \\\"$AGENTMARKET_REQUEST_ID\\\",",
                "\\\"seller\\\": \\\"$AGENTMARKET_SELLER\\\",",
                "\\\"deadline\\\": \\\"$AGENTMARKET_DEADLINE\\\",",
                "\\\"price\\\": \\\"$AGENTMARKET_PRICE\\\"",
                "}\""
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let result = execute_handler(
            script.to_str().unwrap(),
            b"",
            "req-42",
            "0xseller",
            1234567890,
            9900000,
            10,
        )
        .unwrap();

        assert!(
            result.contains("req-42"),
            "should contain request_id, got: {}",
            result
        );
        assert!(
            result.contains("0xseller"),
            "should contain seller, got: {}",
            result
        );
        assert!(
            result.contains("1234567890"),
            "should contain deadline, got: {}",
            result
        );
        assert!(
            result.contains("9900000"),
            "should contain price, got: {}",
            result
        );
    }

    #[test]
    fn test_execute_handler_nonexistent_fails() {
        let result = execute_handler(
            "/nonexistent/path/to/handler",
            b"data",
            "req-1",
            "seller-1",
            1000,
            500,
            5,
        );
        assert!(result.is_err(), "nonexistent handler should fail");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("failed to spawn handler"),
            "error should mention spawn failure, got: {}",
            msg
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_execute_handler_exit_code_nonzero() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("handler.sh");
        fs::write(
            &script,
            "#!/bin/sh\necho 'something went wrong' >&2\nexit 1",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let result = execute_handler(
            script.to_str().unwrap(),
            b"data",
            "req-1",
            "seller-1",
            1000,
            500,
            10,
        );
        assert!(result.is_err(), "non-zero exit should fail");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("exited with code 1"),
            "error should mention exit code, got: {}",
            msg
        );
        assert!(
            msg.contains("something went wrong"),
            "error should include stderr, got: {}",
            msg
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_wait_with_timeout_succeeds() {
        use std::process::Command;

        let child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let output = wait_with_timeout(child, Duration::from_secs(5)).unwrap();
        assert!(output.status.success(), "process should exit successfully");
    }

    #[cfg(unix)]
    #[test]
    fn test_wait_with_timeout_expires() {
        use std::process::Command;

        let child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let result = wait_with_timeout(child, Duration::from_millis(100));
        assert!(result.is_err(), "should time out");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "error should mention timeout, got: {}",
            msg
        );
    }
}
