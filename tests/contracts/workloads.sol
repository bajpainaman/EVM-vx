// SPDX-License-Identifier: MIT
pragma solidity 0.8.35;

// Small benchmark contracts, not audited token/pool implementations.
contract BenchToken {
    mapping(address => uint256) public balanceOf;
    event Transfer(address indexed from, address indexed to, uint256 value);

    constructor() { balanceOf[msg.sender] = 1e24; }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        emit Transfer(msg.sender, to, amount);
        return true;
    }
}

contract BenchProxy {
    // EIP-1967 implementation slot; the harness preloads it outside timing.
    bytes32 constant IMPLEMENTATION =
        0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    fallback() external payable {
        assembly {
            let implementation := sload(IMPLEMENTATION)
            calldatacopy(0, 0, calldatasize())
            let ok := delegatecall(gas(), implementation, 0, calldatasize(), 0, 0)
            returndatacopy(0, 0, returndatasize())
            switch ok
            case 0 { revert(0, returndatasize()) }
            default { return(0, returndatasize()) }
        }
    }
}

contract BenchWorkloads {
    uint256 public reserveIn;
    uint256 public reserveOut;
    event Swap(uint256 amountIn, uint256 amountOut);

    // Constant-product fee/math/storage only; no token accounting or oracle.
    function swap(uint256 amount) external returns (uint256 out) {
        uint256 withFee = amount * 997;
        out = withFee * reserveOut / (reserveIn * 1000 + withFee);
        require(out > 0 && out < reserveOut, "liquidity");
        reserveIn += amount;
        reserveOut -= out;
        emit Swap(amount, out);
    }

    function compound(uint256 principal, uint256 rate, uint256 periods)
        external pure returns (uint256)
    {
        for (uint256 i; i < periods; ++i) principal = principal * rate / 1e18;
        return principal;
    }

    function hashChain(bytes calldata payload, uint256 rounds)
        external pure returns (bytes32 digest)
    {
        for (uint256 i; i < rounds; ++i) {
            digest = keccak256(abi.encodePacked(digest, payload));
        }
    }

    function copy(bytes calldata payload) external pure returns (bytes memory) {
        return payload;
    }

    function sha(bytes calldata payload) external pure returns (bytes32) {
        return sha256(payload);
    }

    function transferAndRevert(BenchToken token, address to, uint256 amount) external {
        require(token.transfer(to, amount), "transfer");
        revert("rollback after transfer");
    }
}
