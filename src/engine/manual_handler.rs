//! Built-in manual validation handler for AgentMarket CLI.
//!
//! Presents deliverables to the terminal operator for interactive
//! approval. The operator sees the task description, seller, price,
//! and deliverable content, then enters a pass/fail decision,
//! quality score, and optional reason.
//!
//! For automated testing, the [`run_manual_review_with_reader`]
//! variant accepts any [`BufRead`] source instead of stdin.

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result};
use tracing::debug;

use crate::engine::requests::format_price_usd;
use crate::engine::validation::{HandlerInput, HandlerOutput};
use crate::output::formatter;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run manual validation: display the deliverable and prompt for approval.
///
/// This is the built-in "manual" handler. It presents the task details
/// and deliverable content to the terminal operator, then asks for a
/// pass/fail decision and an optional score.
///
/// Returns a [`HandlerOutput`] with the operator's decision.
pub fn run_manual_review(input: &HandlerInput) -> Result<HandlerOutput> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    run_manual_review_with_reader(input, &mut reader)
}

/// Testable version that accepts a custom reader for input.
///
/// Identical logic to [`run_manual_review`] but reads from the
/// provided `reader` instead of stdin. This makes the function
/// deterministically testable with pre-canned input.
pub fn run_manual_review_with_reader<R: BufRead>(
    input: &HandlerInput,
    reader: &mut R,
) -> Result<HandlerOutput> {
    // Display request details.
    println!();
    println!("=== Validation Review ===");
    println!();
    formatter::print_info(&format!("Request ID: {}", input.request_id));
    formatter::print_info(&format!("Task: {}", input.task_description));
    formatter::print_info(&format!("Seller: {}", input.seller));
    formatter::print_info(&format!("Price: {}", format_price_usd(input.price_usdc)));
    println!();

    // Display deliverable content.
    println!("--- Deliverable ---");
    match std::str::from_utf8(&input.deliverable) {
        Ok(text) => {
            // Truncate very long content to keep terminal output manageable.
            if text.len() > 5000 {
                println!("{}", &text[..5000]);
                println!("... (truncated, {} bytes total)", text.len());
            } else {
                println!("{}", text);
            }
        }
        Err(_) => {
            println!("[Binary content, {} bytes]", input.deliverable.len());
        }
    }
    println!("--- End Deliverable ---");
    println!();

    // Prompt for pass/fail decision.
    let decision = prompt_line(reader, "Approve? (y/n): ")?;
    let passed = decision.trim().to_lowercase().starts_with('y');

    // Prompt for quality score.
    let score_str = prompt_line(
        reader,
        "Score (0-100, default 80 if approved, 20 if rejected): ",
    )?;
    let score: u8 = if score_str.trim().is_empty() {
        if passed {
            80
        } else {
            20
        }
    } else {
        score_str
            .trim()
            .parse()
            .unwrap_or(if passed { 80 } else { 20 })
    };

    // Prompt for reason.
    let reason_str = prompt_line(reader, "Reason (optional): ")?;
    let reason = if reason_str.trim().is_empty() {
        if passed {
            "Manually approved".to_string()
        } else {
            "Manually rejected".to_string()
        }
    } else {
        reason_str.trim().to_string()
    };

    debug!(
        request_id = %input.request_id,
        passed = passed,
        score = score,
        "manual validation complete"
    );

    Ok(HandlerOutput { score, reason })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Print a prompt to stderr and read a single line from the reader.
///
/// The prompt is written to stderr so it does not interfere with
/// stdout-based output capture in tests or piped workflows.
fn prompt_line<R: BufRead>(reader: &mut R, prompt: &str) -> Result<String> {
    eprint!("{}", prompt);
    io::stderr().flush().context("failed to flush stderr")?;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read input")?;
    Ok(line.trim().to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a sample [`HandlerInput`] for testing.
    fn sample_input() -> HandlerInput {
        HandlerInput {
            request_id: "req-1".to_string(),
            task_description: "Write a test".to_string(),
            deliverable: b"Here is the test code".to_vec(),
            seller: "seller-addr".to_string(),
            price_usdc: 5_000_000,
            deadline: 9_999_999_999,
        }
    }

    #[test]
    fn test_manual_review_approve() {
        let input = sample_input();
        let mut reader = Cursor::new("y\n80\ngood work\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 80);
        assert_eq!(output.reason, "good work");
    }

    #[test]
    fn test_manual_review_reject() {
        let input = sample_input();
        let mut reader = Cursor::new("n\n30\npoor quality\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 30);
        assert_eq!(output.reason, "poor quality");
    }

    #[test]
    fn test_manual_review_approve_defaults() {
        let input = sample_input();
        let mut reader = Cursor::new("y\n\n\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 80);
        assert_eq!(output.reason, "Manually approved");
    }

    #[test]
    fn test_manual_review_reject_defaults() {
        let input = sample_input();
        let mut reader = Cursor::new("n\n\n\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 20);
        assert_eq!(output.reason, "Manually rejected");
    }

    #[test]
    fn test_manual_review_case_insensitive() {
        let input = sample_input();
        let mut reader = Cursor::new("Y\n\n\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 80);
        assert_eq!(output.reason, "Manually approved");
    }

    #[test]
    fn test_manual_review_invalid_score_uses_default() {
        let input = sample_input();
        let mut reader = Cursor::new("y\nabc\n\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        // Invalid score falls back to default for approved (80).
        assert_eq!(output.score, 80);
        assert_eq!(output.reason, "Manually approved");
    }

    #[test]
    fn test_manual_review_binary_deliverable() {
        let mut input = sample_input();
        input.deliverable = vec![0xFF, 0xFE, 0x00, 0x01]; // invalid UTF-8
        let mut reader = Cursor::new("y\n90\nbinary looks fine\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 90);
        assert_eq!(output.reason, "binary looks fine");
    }

    #[test]
    fn test_manual_review_long_deliverable() {
        let mut input = sample_input();
        // Create a deliverable longer than 5000 bytes.
        input.deliverable = "x".repeat(6000).into_bytes();
        let mut reader = Cursor::new("n\n10\ntoo long\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 10);
        assert_eq!(output.reason, "too long");
    }

    #[test]
    fn test_manual_review_yes_word() {
        // "yes" starts with 'y' so should pass.
        let input = sample_input();
        let mut reader = Cursor::new("yes\n95\nexcellent\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 95);
        assert_eq!(output.reason, "excellent");
    }

    #[test]
    fn test_manual_review_no_word() {
        // "no" does not start with 'y' so should fail.
        let input = sample_input();
        let mut reader = Cursor::new("no\n15\nnot acceptable\n");
        let output = run_manual_review_with_reader(&input, &mut reader).unwrap();

        assert_eq!(output.score, 15);
        assert_eq!(output.reason, "not acceptable");
    }

    #[test]
    fn test_prompt_line_reads_trimmed() {
        let mut reader = Cursor::new("  hello world  \n");
        let result = prompt_line(&mut reader, "test: ").unwrap();
        assert_eq!(result, "hello world");
    }
}
