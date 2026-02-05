// SPDX-License-Identifier: MIT
// Deploy: forge script script/Deploy.s.sol --rpc-url $RPC_URL --broadcast --private-key $PRIVATE_KEY
// Base Sepolia USDC: Set USDC_ADDRESS env var to the testnet USDC address
// Base Mainnet USDC: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/RequestRegistry.sol";

contract Deploy is Script {
    function run() external {
        address usdc = vm.envAddress("USDC_ADDRESS");
        address validationRegistry = vm.envAddress("VALIDATION_REGISTRY");
        uint256 validatorFeeBps = vm.envUint("VALIDATOR_FEE_BPS");

        vm.startBroadcast();

        RequestRegistry registry = new RequestRegistry(usdc, validationRegistry, validatorFeeBps);

        vm.stopBroadcast();

        console.log("RequestRegistry deployed at:", address(registry));
    }
}
