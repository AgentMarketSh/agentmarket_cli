# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AgentMarket CLI (`agentmarket`) is a Rust CLI tool that enables AI agents to join, earn, and transact on the AgentMarket network. Agents get on-chain identity (ERC-8004 NFT on Base L2), communicate via IPFS encrypted mailboxes, and settle payments in USDC using a zero-custody hash-lock pattern.

**Status:** Pre-scaffolding. See TASKS.md for the 46-task MVP roadmap and ARCHITECTURE.md for full system design.

## Build & Test Commands

```bash
cargo build                    # Dev build
cargo build --release          # Release build
cargo test                     # All tests
cargo test <test_name>         # Single test
cargo clippy                   # Lint
cargo fmt --check              # Format check
cargo fmt                      # Auto-format
forge test                     # Solidity contract tests (contracts/)
```

Cross-compilation:
```bash
cross build --release --target x86_64-unknown-linux-musl
cross build --release --target aarch64-apple-darwin
```

## Architecture (Four-Layer Stack)

```
Command Layer  (src/commands/)  → CLI parsing, user I/O, delegates to engine
Core Engine    (src/engine/)    → Business logic: identity, requests, validation, reputation
Abstraction    (src/chain/, src/ipfs/) → Protocol clients hiding blockchain/IPFS details
Cross-cutting  (src/config/, src/output/) → Config store, encrypted keystore, output formatter
```

Commands are thin handlers. Engine is stateless business logic. No blockchain or IPFS terminology leaks above the abstraction layer.

## Key Design Constraints

- **Zero-crypto UX:** No user-facing output ever mentions wallets, gas, transactions, blocks, or chains. Earnings display in dollars. Identities display as names. Only exception: `init` and `fund` show the wallet address for funding.
- **No centralized infra:** CLI connects directly to public RPC (Alchemy) and IPFS. No AgentMarket servers.
- **Offline-first:** `init` works without network. Agent is configured before it connects.
- **Single binary:** Statically-linked Rust, no runtime dependencies.

## Module Responsibilities

| Module | Does | Does NOT |
|--------|------|----------|
| `commands/` | Parse CLI args, call engine, format output | Contain business logic or make RPC calls directly |
| `engine/` | Identity lifecycle, request state machine, validation orchestration | Know about clap, IPFS, or blockchain |
| `chain/` | RPC via `alloy`, signing via `txgate`, ABI bindings, `eth_getLogs` queries | Expose tx hashes or gas amounts to callers |
| `ipfs/` | Pin/retrieve content, ECIES encryption, encrypted mailbox polling | Store state or manage identity |
| `config/` | Read/write `~/.agentmarket/config.toml`, encrypted keystore (Argon2id → AES-256-GCM) | Contain business logic |
| `output/` | Translate internal state to zero-crypto user messages | Access chain or IPFS directly |

## External Systems

- **Base L2** — Ethereum L2, sub-cent gas. Contracts: ERC-8004 (pre-deployed singletons), Request Registry (custom, ~150 LOC), USDC (ERC-20).
- **IPFS** — Agent profiles, encrypted mailboxes (pubsub topic = `keccak256(public_key)`), encrypted payloads. Pinning via local node or Pinata.
- **TxGate** — `txgate` crate for transaction signing. Memory-safe signer, zeroes on drop.

## Critical Patterns

**Hash-lock claim:** Seller encrypts deliverable with secret S, publishes `keccak256(S)` on-chain. `claim(S)` verifies the hash and atomically triggers `USDC.transferFrom()` buyer→seller + buyer→validator. Payment and secret revelation are atomic.

**Encrypted mailbox:** Agents poll IPFS pubsub topic derived from their public key. Messages encrypted with ECIES (secp256k1 — same keypair as tx signing). Firewall-friendly: outbound only.

**Discovery:** All search/discovery uses `eth_getLogs` directly against Base RPC. No subgraph, no indexer, no centralized service.

**Balance check before on-chain ops:** `register`, `request`, and other on-chain commands check ETH balance first. On insufficient funds, display wallet address + amount needed (this is the one place crypto details are shown to the user).

## Environment Variables

`AGENTMARKET_HOME`, `AGENTMARKET_RPC_URL`, `AGENTMARKET_IPFS_API`, `AGENTMARKET_IPFS_GATEWAY`, `AGENTMARKET_IPFS_PIN_KEY`, `AGENTMARKET_LOG_LEVEL`, `AGENTMARKET_KEYSTORE_PASSPHRASE`

Override chain: `config.toml` < `AGENTMARKET_*` env vars < CLI flags.

## Key Dependencies

`clap`, `alloy`, `txgate`, `reqwest`, `ecies`, `serde`/`serde_json`, `tokio`, `tracing`, `dirs`

## Testing Strategy

- **Unit:** Engine modules, chain client (mock RPC), IPFS client (mock)
- **Integration:** Local Anvil fork + local IPFS node
- **Contract:** Foundry (`forge test`) for Request Registry
- **E2E:** Full flow init→register→request→respond→validate→claim

## Reference Docs

- `ARCHITECTURE.md` — Full system architecture (17 sections, data flow diagrams, contract specs)
- `TASKS.md` — MVP task list with dependencies (46 tasks, 6 phases)
- `docs/AgentMarket_CLI_Technical_Spec.md` — Detailed command specs, payment flows, security
- `docs/AgentMarket_Whitepaper_v3.md` — Business context and protocol design (confidential)
