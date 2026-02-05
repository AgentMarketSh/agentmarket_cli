// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/RequestRegistry.sol";
import "./MockUSDC.sol";

contract RequestRegistryTest is Test {
    RequestRegistry public registry;
    MockUSDC public usdc;

    address buyer = makeAddr("buyer");
    address seller = makeAddr("seller");
    address validator = makeAddr("validator");
    address validationReg = makeAddr("validationRegistry");

    bytes32 secret = keccak256("my-secret");
    bytes32 secretHash = keccak256(abi.encodePacked(secret));

    uint256 price = 100e6;       // 100 USDC
    uint256 deadline;

    function setUp() public {
        usdc = new MockUSDC();
        registry = new RequestRegistry(address(usdc), validationReg, 500); // 5% fee
        deadline = block.timestamp + 1 days;

        // Fund buyer and approve registry spending
        usdc.mint(buyer, 1000e6);
        vm.prank(buyer);
        usdc.approve(address(registry), type(uint256).max);
    }

    // ---- helpers ----

    function _createRequest() internal returns (uint256) {
        vm.prank(buyer);
        return registry.createRequest("QmRequest123", price, deadline, 0);
    }

    function _createAndRespond() internal returns (uint256) {
        uint256 id = _createRequest();
        vm.prank(seller);
        registry.submitResponse(id, "QmResponse456", secretHash);
        return id;
    }

    function _createRespondAndValidate() internal returns (uint256) {
        uint256 id = _createAndRespond();
        vm.prank(validationReg);
        registry.submitValidation(id, true, validator);
        return id;
    }

    // ---- createRequest ----

    function test_createRequest() public {
        vm.prank(buyer);
        uint256 id = registry.createRequest("QmTest", price, deadline, 0);

        assertEq(id, 0);
        (address b, uint256 p, uint256 d, uint256 t, , ) = registry.requests(id);
        assertEq(b, buyer);
        assertEq(p, price);
        assertEq(d, deadline);
        assertEq(t, 0);
        assertEq(registry.nextRequestId(), 1);
    }

    function test_createRequest_zeroPrice_reverts() public {
        vm.prank(buyer);
        vm.expectRevert("Price must be > 0");
        registry.createRequest("QmTest", 0, deadline, 0);
    }

    function test_createRequest_pastDeadline_reverts() public {
        vm.prank(buyer);
        vm.expectRevert("Deadline must be in the future");
        registry.createRequest("QmTest", price, block.timestamp - 1, 0);
    }

    // ---- submitResponse ----

    function test_submitResponse() public {
        uint256 id = _createRequest();

        vm.prank(seller);
        registry.submitResponse(id, "QmResponse456", secretHash);

        (address s, , bytes32 h) = registry.responses(id);
        assertEq(s, seller);
        assertEq(h, secretHash);

        (, , , , , RequestRegistry.RequestStatus status) = registry.requests(id);
        assertEq(uint8(status), uint8(RequestRegistry.RequestStatus.Responded));
    }

    function test_submitResponse_notOpen_reverts() public {
        uint256 id = _createRequest();

        // First response succeeds
        vm.prank(seller);
        registry.submitResponse(id, "QmResponse456", secretHash);

        // Second response fails — status is now Responded
        vm.prank(seller);
        vm.expectRevert("Request not open");
        registry.submitResponse(id, "QmResponse789", secretHash);
    }

    function test_submitResponse_pastDeadline_reverts() public {
        uint256 id = _createRequest();

        vm.warp(deadline + 1);

        vm.prank(seller);
        vm.expectRevert("Request expired");
        registry.submitResponse(id, "QmResponse456", secretHash);
    }

    // ---- submitValidation ----

    function test_submitValidation() public {
        uint256 id = _createAndRespond();

        vm.prank(validationReg);
        registry.submitValidation(id, true, validator);

        (, , , , , RequestRegistry.RequestStatus status) = registry.requests(id);
        assertEq(uint8(status), uint8(RequestRegistry.RequestStatus.Validated));
        assertEq(registry.validators(id), validator);
    }

    function test_submitValidation_unauthorized_reverts() public {
        uint256 id = _createAndRespond();

        vm.prank(seller);
        vm.expectRevert("Unauthorized");
        registry.submitValidation(id, true, validator);
    }

    // ---- claim ----

    function test_claim_happyPath() public {
        uint256 id = _createRespondAndValidate();

        uint256 buyerBefore = usdc.balanceOf(buyer);
        uint256 sellerBefore = usdc.balanceOf(seller);
        uint256 validatorBefore = usdc.balanceOf(validator);

        vm.prank(seller);
        registry.claim(id, secret);

        (, , , , , RequestRegistry.RequestStatus status) = registry.requests(id);
        assertEq(uint8(status), uint8(RequestRegistry.RequestStatus.Claimed));

        uint256 expectedFee = price * 500 / 10000; // 5%
        uint256 expectedSeller = price - expectedFee;

        assertEq(usdc.balanceOf(buyer), buyerBefore - price);
        assertEq(usdc.balanceOf(seller), sellerBefore + expectedSeller);
        assertEq(usdc.balanceOf(validator), validatorBefore + expectedFee);
    }

    function test_claim_wrongSecret_reverts() public {
        uint256 id = _createRespondAndValidate();

        bytes32 wrongSecret = keccak256("wrong");

        vm.prank(seller);
        vm.expectRevert("Invalid secret");
        registry.claim(id, wrongSecret);
    }

    function test_claim_notValidated_reverts() public {
        uint256 id = _createAndRespond(); // Responded, not validated

        vm.prank(seller);
        vm.expectRevert("Not validated");
        registry.claim(id, secret);
    }

    function test_claim_wrongSeller_reverts() public {
        uint256 id = _createRespondAndValidate();

        vm.prank(buyer); // buyer is not the seller
        vm.expectRevert("Only seller can claim");
        registry.claim(id, secret);
    }

    // ---- cancel ----

    function test_cancel_happyPath() public {
        uint256 id = _createRequest();

        vm.prank(buyer);
        registry.cancel(id);

        (, , , , , RequestRegistry.RequestStatus status) = registry.requests(id);
        assertEq(uint8(status), uint8(RequestRegistry.RequestStatus.Cancelled));
    }

    function test_cancel_notBuyer_reverts() public {
        uint256 id = _createRequest();

        vm.prank(seller);
        vm.expectRevert("Only buyer can cancel");
        registry.cancel(id);
    }

    function test_cancel_notOpen_reverts() public {
        uint256 id = _createAndRespond(); // status = Responded

        vm.prank(buyer);
        vm.expectRevert("Can only cancel open requests");
        registry.cancel(id);
    }

    // ---- expire ----

    function test_expire_happyPath() public {
        uint256 id = _createRequest();

        vm.warp(deadline + 1);

        registry.expire(id);

        (, , , , , RequestRegistry.RequestStatus status) = registry.requests(id);
        assertEq(uint8(status), uint8(RequestRegistry.RequestStatus.Expired));
    }

    function test_expire_notExpired_reverts() public {
        uint256 id = _createRequest();

        vm.expectRevert("Deadline not passed");
        registry.expire(id);
    }

    // ---- full flow e2e ----

    function test_fullFlow() public {
        // 1. Buyer creates request
        vm.prank(buyer);
        uint256 id = registry.createRequest("QmRequestPayload", 50e6, deadline, 0);

        // 2. Seller responds
        bytes32 mySecret = keccak256("deliverable-secret-42");
        bytes32 mySecretHash = keccak256(abi.encodePacked(mySecret));

        vm.prank(seller);
        registry.submitResponse(id, "QmDeliverable", mySecretHash);

        // 3. Validation passes
        vm.prank(validationReg);
        registry.submitValidation(id, true, validator);

        // 4. Seller claims
        uint256 buyerBefore = usdc.balanceOf(buyer);
        uint256 sellerBefore = usdc.balanceOf(seller);
        uint256 validatorBefore = usdc.balanceOf(validator);

        vm.prank(seller);
        registry.claim(id, mySecret);

        // Verify final state
        (, , , , , RequestRegistry.RequestStatus finalStatus) = registry.requests(id);
        assertEq(uint8(finalStatus), uint8(RequestRegistry.RequestStatus.Claimed));

        // Verify USDC transfers: 50 USDC total, 5% = 2.5 USDC fee, 47.5 USDC to seller
        uint256 totalPrice = 50e6;
        uint256 fee = totalPrice * 500 / 10000; // 2_500_000 (2.5 USDC)
        uint256 sellerAmt = totalPrice - fee;    // 47_500_000 (47.5 USDC)

        assertEq(usdc.balanceOf(buyer), buyerBefore - totalPrice);
        assertEq(usdc.balanceOf(seller), sellerBefore + sellerAmt);
        assertEq(usdc.balanceOf(validator), validatorBefore + fee);
    }
}
