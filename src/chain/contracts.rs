//! ABI bindings for on-chain contracts.
//!
//! Uses alloy's `sol!` macro to generate type-safe Rust bindings for the
//! Solidity interfaces the CLI interacts with:
//!
//! - **AgentRegistry** — ERC-8004 identity NFT (register, lookup, URI).
//! - **USDC** — Minimal ERC-20 interface (approve, transferFrom, balanceOf).
//! - **RequestRegistry** — Placeholder for Phase 3 (T-040 / T-043).

use alloy::sol;

// ---------------------------------------------------------------------------
// ERC-8004 Agent Registry
// ---------------------------------------------------------------------------

sol! {
    /// Simplified ERC-8004 Agent Registry interface.
    ///
    /// Each agent mints a soulbound NFT that serves as its on-chain identity.
    /// The `agentURI` points to an IPFS-hosted profile document.
    #[sol(rpc)]
    contract AgentRegistry {
        /// Mint a new agent identity NFT for `msg.sender`.
        function register(string calldata agentURI) external returns (uint256 agentId);

        /// Look up the agent ID owned by `owner`. Returns 0 if none.
        function agentOf(address owner) external view returns (uint256);

        /// Standard ERC-721 owner lookup.
        function ownerOf(uint256 tokenId) external view returns (address);

        /// Retrieve the IPFS URI for an agent's profile.
        function agentURI(uint256 agentId) external view returns (string memory);

        /// Total number of registered agents.
        function totalSupply() external view returns (uint256);

        /// Emitted when a new agent identity is minted.
        event AgentRegistered(uint256 indexed agentId, address indexed owner, string agentURI);

        /// Standard ERC-721 transfer event.
        event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    }
}

// ---------------------------------------------------------------------------
// USDC (ERC-20) — minimal interface for approve + transferFrom
// ---------------------------------------------------------------------------

sol! {
    /// Minimal ERC-20 interface for USDC interactions.
    ///
    /// Only the functions needed by the payment flow are included:
    /// approve (buyer grants allowance), transferFrom (contract pulls funds),
    /// balanceOf, allowance, and decimals.
    #[sol(rpc)]
    contract USDC {
        /// Approve `spender` to transfer up to `amount` tokens on behalf of the caller.
        function approve(address spender, uint256 amount) external returns (bool);

        /// Transfer `amount` tokens from `from` to `to` (requires prior approval).
        function transferFrom(address from, address to, uint256 amount) external returns (bool);

        /// Query the token balance of `account`.
        function balanceOf(address account) external view returns (uint256);

        /// Query the remaining allowance that `spender` can transfer from `owner`.
        function allowance(address owner, address spender) external view returns (uint256);

        /// Number of decimal places (6 for USDC).
        function decimals() external view returns (uint8);

        /// Emitted when an allowance is set via `approve`.
        event Approval(address indexed owner, address indexed spender, uint256 value);

        /// Emitted on token transfer.
        event Transfer(address indexed from, address indexed to, uint256 value);
    }
}

// ---------------------------------------------------------------------------
// Request Registry — Phase 3 placeholder (T-040 / T-043)
// ---------------------------------------------------------------------------

// The Request Registry contract handles the full request lifecycle:
// create, respond, validate, claim (hash-lock pattern), cancel, and expire.
//
// ABI bindings will be added here once the Solidity contract is written and
// deployed in Phase 3. See TASKS.md tasks T-040 and T-043 for details.
//
// Expected interface (preview):
//
//   function createRequest(...) external returns (uint256 requestId);
//   function respond(uint256 requestId, ...) external;
//   function validate(uint256 requestId, ...) external;
//   function claim(uint256 requestId, bytes32 secret) external;
//   function cancel(uint256 requestId) external;
//   event RequestCreated(...);
//   event ResponseSubmitted(...);
//   event RequestValidated(...);
//   event RequestClaimed(...);

// ---------------------------------------------------------------------------
// Known contract addresses on Base mainnet
// ---------------------------------------------------------------------------

/// Deployed contract addresses on Base mainnet.
///
/// Placeholder addresses (`0x0000...0000`) indicate contracts that have not
/// yet been deployed. They will be updated before mainnet launch.
pub mod addresses {
    use alloy::primitives::{address, Address};

    /// USDC on Base mainnet.
    pub const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

    /// ERC-8004 Agent Registry on Base mainnet (placeholder -- to be updated after deployment).
    pub const AGENT_REGISTRY: Address = address!("0000000000000000000000000000000000000000");

    /// Request Registry on Base mainnet (placeholder -- Phase 3 deployment).
    pub const REQUEST_REGISTRY: Address = address!("0000000000000000000000000000000000000000");

    /// USDC uses 6 decimal places.
    pub const USDC_DECIMALS: u8 = 6;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

    // -- Address constants ------------------------------------------------

    #[test]
    fn usdc_address_is_correct_base_mainnet() {
        // Base mainnet USDC: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913
        let expected: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
            .parse()
            .unwrap();
        assert_eq!(addresses::USDC, expected);
    }

    #[test]
    fn agent_registry_address_is_placeholder() {
        assert_eq!(addresses::AGENT_REGISTRY, Address::ZERO);
    }

    #[test]
    fn request_registry_address_is_placeholder() {
        assert_eq!(addresses::REQUEST_REGISTRY, Address::ZERO);
    }

    #[test]
    fn usdc_decimals_is_six() {
        assert_eq!(addresses::USDC_DECIMALS, 6);
    }

    // -- AgentRegistry sol! type generation --------------------------------

    #[test]
    fn agent_registry_register_call_can_be_constructed() {
        // Verify that the sol! macro generated the `registerCall` struct.
        let call = AgentRegistry::registerCall {
            agentURI: "ipfs://QmTest123".to_string(),
        };
        assert_eq!(call.agentURI, "ipfs://QmTest123");
    }

    #[test]
    fn agent_registry_agent_of_call_can_be_constructed() {
        let call = AgentRegistry::agentOfCall {
            owner: Address::ZERO,
        };
        assert_eq!(call.owner, Address::ZERO);
    }

    #[test]
    fn agent_registry_owner_of_call_can_be_constructed() {
        let call = AgentRegistry::ownerOfCall {
            tokenId: U256::from(1u64),
        };
        assert_eq!(call.tokenId, U256::from(1u64));
    }

    #[test]
    fn agent_registry_agent_uri_call_can_be_constructed() {
        let call = AgentRegistry::agentURICall {
            agentId: U256::from(42u64),
        };
        assert_eq!(call.agentId, U256::from(42u64));
    }

    #[test]
    fn agent_registry_total_supply_call_can_be_constructed() {
        let _call = AgentRegistry::totalSupplyCall {};
    }

    #[test]
    fn agent_registry_registered_event_can_be_constructed() {
        let event = AgentRegistry::AgentRegistered {
            agentId: U256::from(1u64),
            owner: Address::ZERO,
            agentURI: "ipfs://QmTest".to_string(),
        };
        assert_eq!(event.agentId, U256::from(1u64));
    }

    // -- USDC sol! type generation ----------------------------------------

    #[test]
    fn usdc_approve_call_can_be_constructed() {
        let call = USDC::approveCall {
            spender: Address::ZERO,
            amount: U256::from(1_000_000u64),
        };
        assert_eq!(call.amount, U256::from(1_000_000u64));
    }

    #[test]
    fn usdc_transfer_from_call_can_be_constructed() {
        let call = USDC::transferFromCall {
            from: Address::ZERO,
            to: Address::ZERO,
            amount: U256::from(500_000u64),
        };
        assert_eq!(call.amount, U256::from(500_000u64));
    }

    #[test]
    fn usdc_balance_of_call_can_be_constructed() {
        let call = USDC::balanceOfCall {
            account: Address::ZERO,
        };
        assert_eq!(call.account, Address::ZERO);
    }

    #[test]
    fn usdc_allowance_call_can_be_constructed() {
        let call = USDC::allowanceCall {
            owner: Address::ZERO,
            spender: Address::ZERO,
        };
        assert_eq!(call.owner, Address::ZERO);
        assert_eq!(call.spender, Address::ZERO);
    }

    #[test]
    fn usdc_decimals_call_can_be_constructed() {
        let _call = USDC::decimalsCall {};
    }

    #[test]
    fn usdc_transfer_event_can_be_constructed() {
        let event = USDC::Transfer {
            from: Address::ZERO,
            to: Address::ZERO,
            value: U256::from(1_000_000u64),
        };
        assert_eq!(event.value, U256::from(1_000_000u64));
    }

    #[test]
    fn usdc_approval_event_can_be_constructed() {
        let event = USDC::Approval {
            owner: Address::ZERO,
            spender: Address::ZERO,
            value: U256::from(1_000_000u64),
        };
        assert_eq!(event.value, U256::from(1_000_000u64));
    }
}
