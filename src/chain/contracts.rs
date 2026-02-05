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
// Request Registry — zero-custody marketplace for agent service requests
// ---------------------------------------------------------------------------

sol! {
    /// Request Registry — zero-custody marketplace for agent service requests.
    ///
    /// Handles the full request lifecycle: create, respond, validate,
    /// claim (hash-lock pattern), cancel, and expire. Payments settle
    /// atomically in USDC via the hash-lock `claim` function.
    #[sol(rpc)]
    contract RequestRegistry {
        /// Status of a request through its lifecycle.
        enum RequestStatus {
            Open,
            Responded,
            Validated,
            Claimed,
            Cancelled,
            Expired
        }

        /// A buyer's service request.
        struct Request {
            address buyer;
            uint256 price;
            uint256 deadline;
            uint256 targetAgentId;
            string ipfsCid;
            RequestStatus status;
        }

        /// A seller's response to a request.
        struct Response {
            address seller;
            string ipfsCid;
            bytes32 secretHash;
        }

        /// USDC token used for payment settlement.
        address public usdc;

        /// Address of the validation registry contract.
        address public validationRegistry;

        /// Auto-incrementing request ID counter.
        uint256 public nextRequestId;

        /// Validator fee in basis points (e.g., 500 = 5%).
        uint256 public validatorFeeBps;

        /// Mapping from request ID to Request data.
        mapping(uint256 => Request) public requests;

        /// Mapping from request ID to Response data.
        mapping(uint256 => Response) public responses;

        /// Mapping from request ID to assigned validator address.
        mapping(uint256 => address) public validators;

        /// Create a new service request. Caller becomes the buyer.
        function createRequest(string calldata ipfsCid, uint256 price, uint256 deadline, uint256 targetAgentId) external returns (uint256 requestId);

        /// Submit a response to an open request. Caller becomes the seller.
        function submitResponse(uint256 requestId, string calldata ipfsCid, bytes32 secretHash) external;

        /// Submit a validation result for a responded request.
        function submitValidation(uint256 requestId, bool passed, address validator) external;

        /// Claim payment by revealing the secret. Atomically settles USDC.
        function claim(uint256 requestId, bytes32 secret) external;

        /// Cancel an open request. Only the buyer can cancel.
        function cancel(uint256 requestId) external;

        /// Expire a request that has passed its deadline.
        function expire(uint256 requestId) external;

        /// Emitted when a new request is created.
        event RequestCreated(uint256 indexed requestId, address indexed buyer, uint256 price, uint256 deadline);

        /// Emitted when a seller submits a response.
        event ResponseSubmitted(uint256 indexed requestId, address indexed seller, bytes32 secretHash);

        /// Emitted when a request passes or fails validation.
        event RequestValidated(uint256 indexed requestId, bool passed, address validator);

        /// Emitted when payment is claimed by revealing the secret.
        event RequestClaimed(uint256 indexed requestId, bytes32 secret);

        /// Emitted when a request is cancelled by the buyer.
        event RequestCancelled(uint256 indexed requestId);

        /// Emitted when a request expires past its deadline.
        event RequestExpired(uint256 indexed requestId);
    }
}

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

    // -- RequestRegistry sol! type generation --------------------------------

    #[test]
    fn test_request_registry_create_request_call() {
        let call = RequestRegistry::createRequestCall {
            ipfsCid: "ipfs://QmRequestCid".to_string(),
            price: U256::from(5_000_000u64), // $5 USDC
            deadline: U256::from(1_700_000_000u64),
            targetAgentId: U256::from(42u64),
        };
        assert_eq!(call.ipfsCid, "ipfs://QmRequestCid");
        assert_eq!(call.price, U256::from(5_000_000u64));
        assert_eq!(call.deadline, U256::from(1_700_000_000u64));
        assert_eq!(call.targetAgentId, U256::from(42u64));
    }

    #[test]
    fn test_request_registry_submit_response_call() {
        use alloy::primitives::B256;
        let call = RequestRegistry::submitResponseCall {
            requestId: U256::from(1u64),
            ipfsCid: "ipfs://QmResponseCid".to_string(),
            secretHash: B256::ZERO,
        };
        assert_eq!(call.requestId, U256::from(1u64));
        assert_eq!(call.ipfsCid, "ipfs://QmResponseCid");
        assert_eq!(call.secretHash, B256::ZERO);
    }

    #[test]
    fn test_request_registry_claim_call() {
        use alloy::primitives::B256;
        let call = RequestRegistry::claimCall {
            requestId: U256::from(1u64),
            secret: B256::ZERO,
        };
        assert_eq!(call.requestId, U256::from(1u64));
        assert_eq!(call.secret, B256::ZERO);
    }

    #[test]
    fn test_request_registry_cancel_call() {
        let call = RequestRegistry::cancelCall {
            requestId: U256::from(7u64),
        };
        assert_eq!(call.requestId, U256::from(7u64));
    }

    #[test]
    fn test_request_registry_expire_call() {
        let call = RequestRegistry::expireCall {
            requestId: U256::from(99u64),
        };
        assert_eq!(call.requestId, U256::from(99u64));
    }

    #[test]
    fn test_request_registry_submit_validation_call() {
        let call = RequestRegistry::submitValidationCall {
            requestId: U256::from(3u64),
            passed: true,
            validator: Address::ZERO,
        };
        assert_eq!(call.requestId, U256::from(3u64));
        assert!(call.passed);
        assert_eq!(call.validator, Address::ZERO);
    }

    #[test]
    fn test_request_registry_request_created_event() {
        let event = RequestRegistry::RequestCreated {
            requestId: U256::from(1u64),
            buyer: Address::ZERO,
            price: U256::from(10_000_000u64),
            deadline: U256::from(1_700_000_000u64),
        };
        assert_eq!(event.requestId, U256::from(1u64));
        assert_eq!(event.buyer, Address::ZERO);
        assert_eq!(event.price, U256::from(10_000_000u64));
        assert_eq!(event.deadline, U256::from(1_700_000_000u64));
    }

    #[test]
    fn test_request_registry_response_submitted_event() {
        use alloy::primitives::B256;
        let event = RequestRegistry::ResponseSubmitted {
            requestId: U256::from(2u64),
            seller: Address::ZERO,
            secretHash: B256::ZERO,
        };
        assert_eq!(event.requestId, U256::from(2u64));
        assert_eq!(event.seller, Address::ZERO);
        assert_eq!(event.secretHash, B256::ZERO);
    }

    #[test]
    fn test_request_registry_request_claimed_event() {
        use alloy::primitives::B256;
        let event = RequestRegistry::RequestClaimed {
            requestId: U256::from(5u64),
            secret: B256::ZERO,
        };
        assert_eq!(event.requestId, U256::from(5u64));
        assert_eq!(event.secret, B256::ZERO);
    }
}
