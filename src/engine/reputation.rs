use serde::{Deserialize, Serialize};

/// A single validation record used for reputation computation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationRecord {
    /// The request ID this validation is for.
    pub request_id: String,
    /// Whether the validation passed.
    pub passed: bool,
    /// Unix timestamp of the validation.
    pub timestamp: u64,
    /// The validator address.
    pub validator: String,
}

/// Computed reputation score for an agent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReputationScore {
    /// Agent address or ID.
    pub agent_id: String,
    /// Number of completed (claimed) requests.
    pub completed_requests: u64,
    /// Number of failed validations.
    pub failed_validations: u64,
    /// Total earnings in USDC (6 decimals).
    pub total_earnings_usdc: u64,
    /// Reputation score (0.0 to 100.0).
    pub score: f64,
    /// Average response time in seconds (0 if unknown).
    pub avg_response_time: u64,
}

/// Compute a reputation score from validation records and earnings data.
///
/// The scoring formula:
/// - Base score = (completed / (completed + failed)) * 100
/// - If no records, score = 0.0
/// - Minimum 1 completed request to get a non-zero score
///
/// This is a pure computation -- no I/O.
pub fn compute_reputation(
    agent_id: &str,
    records: &[ValidationRecord],
    total_earnings_usdc: u64,
    avg_response_time: u64,
) -> ReputationScore {
    let completed = records.iter().filter(|r| r.passed).count() as u64;
    let failed = records.iter().filter(|r| !r.passed).count() as u64;

    let score = if records.is_empty() || completed == 0 {
        0.0
    } else {
        (completed as f64 / (completed + failed) as f64) * 100.0
    };

    ReputationScore {
        agent_id: agent_id.to_string(),
        completed_requests: completed,
        failed_validations: failed,
        total_earnings_usdc,
        score,
        avg_response_time,
    }
}

/// Format a reputation score as a human-readable string.
/// e.g., "97.3" or "N/A" if no records.
pub fn format_reputation(score: &ReputationScore) -> String {
    if score.completed_requests == 0 && score.failed_validations == 0 {
        "N/A".to_string()
    } else {
        format!("{:.1}", score.score)
    }
}

/// Format total earnings as USD.
/// e.g., "$42.50"
pub fn format_earnings_usd(usdc_amount: u64) -> String {
    let dollars = usdc_amount / 1_000_000;
    let cents = (usdc_amount % 1_000_000) / 10_000;
    format!("${}.{:02}", dollars, cents)
}

/// Get a reputation tier based on score.
/// - >= 95: "Excellent"
/// - >= 80: "Good"
/// - >= 60: "Fair"
/// - >= 0:  "New"
/// - No records: "Unrated"
pub fn reputation_tier(score: &ReputationScore) -> &'static str {
    if score.completed_requests == 0 && score.failed_validations == 0 {
        return "Unrated";
    }
    if score.score >= 95.0 {
        "Excellent"
    } else if score.score >= 80.0 {
        "Good"
    } else if score.score >= 60.0 {
        "Fair"
    } else {
        "New"
    }
}

/// Summary used by the status command.
#[derive(Clone, Debug)]
pub struct AgentSummary {
    pub agent_id: String,
    pub name: String,
    pub reputation: ReputationScore,
    pub active_requests: u64,
    pub pending_validations: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(request_id: &str, passed: bool) -> ValidationRecord {
        ValidationRecord {
            request_id: request_id.to_string(),
            passed,
            timestamp: 1_700_000_000,
            validator: "0xvalidator".to_string(),
        }
    }

    #[test]
    fn test_compute_reputation_no_records() {
        let score = compute_reputation("agent1", &[], 0, 0);
        assert_eq!(score.score, 0.0);
        assert_eq!(score.completed_requests, 0);
        assert_eq!(score.failed_validations, 0);
    }

    #[test]
    fn test_compute_reputation_all_passed() {
        let records: Vec<ValidationRecord> = (0..10)
            .map(|i| make_record(&format!("r{}", i), true))
            .collect();
        let score = compute_reputation("agent1", &records, 50_000_000, 120);
        assert_eq!(score.score, 100.0);
        assert_eq!(score.completed_requests, 10);
        assert_eq!(score.failed_validations, 0);
    }

    #[test]
    fn test_compute_reputation_mixed() {
        let mut records: Vec<ValidationRecord> = (0..8)
            .map(|i| make_record(&format!("r{}", i), true))
            .collect();
        records.extend((8..10).map(|i| make_record(&format!("r{}", i), false)));
        let score = compute_reputation("agent1", &records, 40_000_000, 200);
        assert!((score.score - 80.0).abs() < f64::EPSILON);
        assert_eq!(score.completed_requests, 8);
        assert_eq!(score.failed_validations, 2);
    }

    #[test]
    fn test_compute_reputation_all_failed() {
        let records: Vec<ValidationRecord> = (0..5)
            .map(|i| make_record(&format!("r{}", i), false))
            .collect();
        let score = compute_reputation("agent1", &records, 0, 0);
        assert_eq!(score.score, 0.0);
        assert_eq!(score.completed_requests, 0);
        assert_eq!(score.failed_validations, 5);
    }

    #[test]
    fn test_compute_reputation_single_pass() {
        let records = vec![make_record("r0", true)];
        let score = compute_reputation("agent1", &records, 5_000_000, 60);
        assert_eq!(score.score, 100.0);
        assert_eq!(score.completed_requests, 1);
        assert_eq!(score.failed_validations, 0);
    }

    #[test]
    fn test_format_reputation_with_score() {
        let score = compute_reputation(
            "agent1",
            &[
                make_record("r0", true),
                make_record("r1", true),
                make_record("r2", false),
            ],
            0,
            0,
        );
        let formatted = format_reputation(&score);
        assert_eq!(formatted, "66.7");
    }

    #[test]
    fn test_format_reputation_no_records() {
        let score = compute_reputation("agent1", &[], 0, 0);
        let formatted = format_reputation(&score);
        assert_eq!(formatted, "N/A");
    }

    #[test]
    fn test_format_earnings_usd() {
        assert_eq!(format_earnings_usd(0), "$0.00");
        assert_eq!(format_earnings_usd(42_500_000), "$42.50");
        assert_eq!(format_earnings_usd(1_000_000), "$1.00");
        assert_eq!(format_earnings_usd(500_000), "$0.50");
        assert_eq!(format_earnings_usd(100_230_000), "$100.23");
    }

    #[test]
    fn test_reputation_tier_excellent() {
        let records: Vec<ValidationRecord> = (0..20)
            .map(|i| make_record(&format!("r{}", i), true))
            .collect();
        let score = compute_reputation("agent1", &records, 0, 0);
        assert_eq!(reputation_tier(&score), "Excellent");
    }

    #[test]
    fn test_reputation_tier_good() {
        let mut records: Vec<ValidationRecord> = (0..8)
            .map(|i| make_record(&format!("r{}", i), true))
            .collect();
        records.extend((8..10).map(|i| make_record(&format!("r{}", i), false)));
        let score = compute_reputation("agent1", &records, 0, 0);
        assert_eq!(reputation_tier(&score), "Good");
    }

    #[test]
    fn test_reputation_tier_fair() {
        let mut records: Vec<ValidationRecord> = (0..6)
            .map(|i| make_record(&format!("r{}", i), true))
            .collect();
        records.extend((6..10).map(|i| make_record(&format!("r{}", i), false)));
        let score = compute_reputation("agent1", &records, 0, 0);
        assert_eq!(reputation_tier(&score), "Fair");
    }

    #[test]
    fn test_reputation_tier_new() {
        let mut records: Vec<ValidationRecord> = (0..3)
            .map(|i| make_record(&format!("r{}", i), true))
            .collect();
        records.extend((3..10).map(|i| make_record(&format!("r{}", i), false)));
        let score = compute_reputation("agent1", &records, 0, 0);
        // 30% score < 60 -> "New"
        assert_eq!(reputation_tier(&score), "New");
    }

    #[test]
    fn test_reputation_tier_unrated() {
        let score = compute_reputation("agent1", &[], 0, 0);
        assert_eq!(reputation_tier(&score), "Unrated");
    }
}
