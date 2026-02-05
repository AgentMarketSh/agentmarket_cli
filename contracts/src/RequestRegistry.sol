// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";

/// @title RequestRegistry
/// @notice Zero-custody request marketplace. Routes USDC via transferFrom; never holds funds.
contract RequestRegistry {
    enum RequestStatus { Open, Responded, Validated, Claimed, Cancelled, Expired }

    struct Request {
        address buyer;
        uint256 price;          // USDC amount (6 decimals)
        uint256 deadline;       // block.timestamp expiry
        uint256 targetAgentId;  // 0 = open request
        string ipfsCid;         // Encrypted request payload
        RequestStatus status;
    }

    struct Response {
        address seller;
        string ipfsCid;         // Encrypted deliverable
        bytes32 secretHash;     // keccak256(secret)
    }

    // --- Storage ---
    IERC20 public usdc;
    address public validationRegistry;
    uint256 public nextRequestId;
    uint256 public validatorFeeBps;

    mapping(uint256 => Request) public requests;
    mapping(uint256 => Response) public responses;
    mapping(uint256 => address) public validators;

    // --- Events ---
    event RequestCreated(uint256 indexed requestId, address indexed buyer, uint256 price, uint256 deadline);
    event ResponseSubmitted(uint256 indexed requestId, address indexed seller, bytes32 secretHash);
    event RequestValidated(uint256 indexed requestId, bool passed, address validator);
    event RequestClaimed(uint256 indexed requestId, bytes32 secret);
    event RequestCancelled(uint256 indexed requestId);
    event RequestExpired(uint256 indexed requestId);

    constructor(address _usdc, address _validationRegistry, uint256 _validatorFeeBps) {
        usdc = IERC20(_usdc);
        validationRegistry = _validationRegistry;
        validatorFeeBps = _validatorFeeBps;
    }

    /// @notice Create a new request.
    function createRequest(
        string calldata ipfsCid,
        uint256 price,
        uint256 deadline,
        uint256 targetAgentId
    ) external returns (uint256 requestId) {
        require(price > 0, "Price must be > 0");
        require(deadline > block.timestamp, "Deadline must be in the future");

        requestId = nextRequestId++;
        requests[requestId] = Request({
            buyer: msg.sender,
            price: price,
            deadline: deadline,
            targetAgentId: targetAgentId,
            ipfsCid: ipfsCid,
            status: RequestStatus.Open
        });

        emit RequestCreated(requestId, msg.sender, price, deadline);
    }

    /// @notice Submit a response to an open request.
    function submitResponse(
        uint256 requestId,
        string calldata ipfsCid,
        bytes32 secretHash
    ) external {
        Request storage req = requests[requestId];
        require(req.status == RequestStatus.Open, "Request not open");
        require(block.timestamp <= req.deadline, "Request expired");

        responses[requestId] = Response({
            seller: msg.sender,
            ipfsCid: ipfsCid,
            secretHash: secretHash
        });
        req.status = RequestStatus.Responded;

        emit ResponseSubmitted(requestId, msg.sender, secretHash);
    }

    /// @notice Submit validation result. Only callable by the validation registry.
    function submitValidation(
        uint256 requestId,
        bool passed,
        address validator
    ) external {
        require(msg.sender == validationRegistry, "Unauthorized");
        require(requests[requestId].status == RequestStatus.Responded, "Not responded");

        if (passed) {
            requests[requestId].status = RequestStatus.Validated;
            validators[requestId] = validator;
        }

        emit RequestValidated(requestId, passed, validator);
    }

    /// @notice Claim payment by revealing the secret. Atomically transfers USDC.
    function claim(uint256 requestId, bytes32 secret) external {
        Request storage req = requests[requestId];
        Response storage resp = responses[requestId];

        require(req.status == RequestStatus.Validated, "Not validated");
        require(keccak256(abi.encodePacked(secret)) == resp.secretHash, "Invalid secret");
        require(msg.sender == resp.seller, "Only seller can claim");
        require(block.timestamp <= req.deadline, "Request expired");

        uint256 validatorFee = req.price * validatorFeeBps / 10000;
        uint256 sellerPayment = req.price - validatorFee;

        req.status = RequestStatus.Claimed;

        require(usdc.transferFrom(req.buyer, resp.seller, sellerPayment), "Seller transfer failed");
        require(usdc.transferFrom(req.buyer, validators[requestId], validatorFee), "Validator transfer failed");

        emit RequestClaimed(requestId, secret);
    }

    /// @notice Cancel an open request. Only the buyer can cancel.
    function cancel(uint256 requestId) external {
        Request storage req = requests[requestId];
        require(msg.sender == req.buyer, "Only buyer can cancel");
        require(req.status == RequestStatus.Open, "Can only cancel open requests");

        req.status = RequestStatus.Cancelled;

        emit RequestCancelled(requestId);
    }

    /// @notice Expire a request past its deadline.
    function expire(uint256 requestId) external {
        Request storage req = requests[requestId];
        require(block.timestamp > req.deadline, "Deadline not passed");
        require(
            req.status == RequestStatus.Open ||
            req.status == RequestStatus.Responded ||
            req.status == RequestStatus.Validated,
            "Cannot expire"
        );

        req.status = RequestStatus.Expired;

        emit RequestExpired(requestId);
    }
}
