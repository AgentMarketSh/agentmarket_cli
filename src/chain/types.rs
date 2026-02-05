use std::fmt;

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Core wrapper types
// ---------------------------------------------------------------------------

/// Wrapper around [`U256`] for type-safe agent identifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub U256);

/// Wrapper around [`U256`] for type-safe request identifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub U256);

// ---------------------------------------------------------------------------
// From conversions
// ---------------------------------------------------------------------------

impl From<U256> for AgentId {
    fn from(val: U256) -> Self {
        Self(val)
    }
}

impl From<U256> for RequestId {
    fn from(val: U256) -> Self {
        Self(val)
    }
}

// ---------------------------------------------------------------------------
// Display — decimal string representation
// ---------------------------------------------------------------------------

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// RequestStatus
// ---------------------------------------------------------------------------

/// Request status on-chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestStatus {
    Open,
    Responded,
    Validated,
    Claimed,
    Expired,
    Cancelled,
}

impl RequestStatus {
    /// Map a `u8` discriminant (as stored on-chain) to a [`RequestStatus`].
    ///
    /// Returns `None` for unrecognised values.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Open),
            1 => Some(Self::Responded),
            2 => Some(Self::Validated),
            3 => Some(Self::Claimed),
            4 => Some(Self::Expired),
            5 => Some(Self::Cancelled),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// On-chain request data.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: RequestId,
    pub buyer: Address,
    pub buyer_agent_id: AgentId,
    pub seller_agent_id: Option<AgentId>,
    pub ipfs_cid: String,
    pub price_wei: U256,
    pub deadline: u64,
    pub status: RequestStatus,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// On-chain response data.
#[derive(Clone, Debug)]
pub struct Response {
    pub request_id: RequestId,
    pub seller: Address,
    pub seller_agent_id: AgentId,
    pub ipfs_cid: String,
    pub secret_hash: [u8; 32],
}

// ---------------------------------------------------------------------------
// Balance
// ---------------------------------------------------------------------------

/// ETH balance information.
///
/// Field names intentionally avoid raw blockchain terminology so that
/// higher layers can stay "zero-crypto" in their UX.
#[derive(Clone, Debug)]
pub struct Balance {
    pub wei: U256,
}

/// Minimum balance required for a registration transaction on Base L2.
/// 0.0001 ETH = 100_000 gwei = 100_000_000_000_000 wei.
const REGISTRATION_MIN_WEI: u128 = 100_000_000_000_000; // 1e14

impl Balance {
    /// Returns `true` when the balance is enough to cover the gas cost of
    /// an agent registration on Base L2 (>= 0.0001 ETH).
    pub fn is_sufficient_for_registration(&self) -> bool {
        self.wei >= U256::from(REGISTRATION_MIN_WEI)
    }

    /// Human-readable ETH representation, e.g. `"0.0001 ETH"`.
    ///
    /// Displays exactly 4 decimal places.
    pub fn display_eth(&self) -> String {
        const ETH: u128 = 1_000_000_000_000_000_000; // 1e18

        // Integer division: whole = wei / 1e18, frac = wei % 1e18
        let whole = self.wei / U256::from(ETH);
        let remainder = self.wei % U256::from(ETH);

        // Scale remainder to 4 decimal digits: remainder * 10_000 / 1e18
        let frac = (remainder * U256::from(10_000u64)) / U256::from(ETH);

        format!("{}.{:04} ETH", whole, frac.to::<u64>())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- RequestStatus::from_u8 ------------------------------------------

    #[test]
    fn request_status_from_u8_valid() {
        assert_eq!(RequestStatus::from_u8(0), Some(RequestStatus::Open));
        assert_eq!(RequestStatus::from_u8(1), Some(RequestStatus::Responded));
        assert_eq!(RequestStatus::from_u8(2), Some(RequestStatus::Validated));
        assert_eq!(RequestStatus::from_u8(3), Some(RequestStatus::Claimed));
        assert_eq!(RequestStatus::from_u8(4), Some(RequestStatus::Expired));
        assert_eq!(RequestStatus::from_u8(5), Some(RequestStatus::Cancelled));
    }

    #[test]
    fn request_status_from_u8_invalid() {
        assert_eq!(RequestStatus::from_u8(6), None);
        assert_eq!(RequestStatus::from_u8(255), None);
    }

    // -- Balance::is_sufficient_for_registration --------------------------

    #[test]
    fn balance_sufficient_exactly_at_threshold() {
        let balance = Balance {
            wei: U256::from(REGISTRATION_MIN_WEI),
        };
        assert!(balance.is_sufficient_for_registration());
    }

    #[test]
    fn balance_sufficient_above_threshold() {
        let balance = Balance {
            wei: U256::from(REGISTRATION_MIN_WEI + 1),
        };
        assert!(balance.is_sufficient_for_registration());
    }

    #[test]
    fn balance_insufficient_below_threshold() {
        let balance = Balance {
            wei: U256::from(REGISTRATION_MIN_WEI - 1),
        };
        assert!(!balance.is_sufficient_for_registration());
    }

    #[test]
    fn balance_insufficient_zero() {
        let balance = Balance { wei: U256::ZERO };
        assert!(!balance.is_sufficient_for_registration());
    }

    // -- Balance::display_eth ---------------------------------------------

    #[test]
    fn display_eth_zero() {
        let balance = Balance { wei: U256::ZERO };
        assert_eq!(balance.display_eth(), "0.0000 ETH");
    }

    #[test]
    fn display_eth_one_ether() {
        let balance = Balance {
            wei: U256::from(1_000_000_000_000_000_000u128),
        };
        assert_eq!(balance.display_eth(), "1.0000 ETH");
    }

    #[test]
    fn display_eth_threshold() {
        let balance = Balance {
            wei: U256::from(REGISTRATION_MIN_WEI),
        };
        assert_eq!(balance.display_eth(), "0.0001 ETH");
    }

    #[test]
    fn display_eth_fractional() {
        // 1.5 ETH = 1_500_000_000_000_000_000 wei
        let balance = Balance {
            wei: U256::from(1_500_000_000_000_000_000u128),
        };
        assert_eq!(balance.display_eth(), "1.5000 ETH");
    }

    // -- Display impls ----------------------------------------------------

    #[test]
    fn agent_id_display() {
        let id = AgentId(U256::from(42));
        assert_eq!(format!("{id}"), "42");
    }

    #[test]
    fn request_id_display() {
        let id = RequestId(U256::from(1337));
        assert_eq!(format!("{id}"), "1337");
    }

    // -- From<U256> conversions -------------------------------------------

    #[test]
    fn agent_id_from_u256() {
        let val = U256::from(99);
        let id: AgentId = val.into();
        assert_eq!(id.0, U256::from(99));
    }

    #[test]
    fn request_id_from_u256() {
        let val = U256::from(7);
        let id: RequestId = val.into();
        assert_eq!(id.0, U256::from(7));
    }
}
