# AgentMarket CLI — Task List (MVP)

Stripped to the essentials. Ship the core loop: **init → fund → register → search → request → respond → validate → claim → status → withdraw**.

**Legend:** `[ ]` pending | `[x]` done | `[~]` in progress

---

## Phase 0: Project Scaffolding

- [ ] **T-001** Initialize Rust project (`cargo init`), configure `Cargo.toml`, set up module structure (`commands/`, `engine/`, `chain/`, `ipfs/`, `config/`, `output/`), add dependencies (`clap`, `tokio`, `serde`, `serde_json`, `tracing`, `dirs`, `reqwest`, `alloy`, `txgate`, `ecies`)
- [ ] **T-002** Set up `clap` CLI skeleton in `main.rs` with all subcommands stubbed: `init`, `register`, `search`, `request`, `respond`, `validate`, `claim`, `status`, `fund`, `withdraw`, `daemon`
- [ ] **T-003** Set up CI (GitHub Actions: `cargo test`, `cargo clippy`, `cargo fmt --check`)
- [ ] **T-004** Configure `tracing` for debug logging (never user-facing)

---

## Phase 1: Local Identity & Config (offline)

- [ ] **T-010** Implement `config/store.rs` — read/write `~/.agentmarket/config.toml`, `serde` typed config struct, env var overrides (`AGENTMARKET_HOME`, `AGENTMARKET_RPC_URL`, `AGENTMARKET_IPFS_API`, `AGENTMARKET_IPFS_GATEWAY`, `AGENTMARKET_IPFS_PIN_KEY`, `AGENTMARKET_LOG_LEVEL`, `AGENTMARKET_KEYSTORE_PASSPHRASE`)
- [ ] **T-011** Implement `config/keystore.rs` — encrypted private key storage (Argon2id KDF → AES-256-GCM), passphrase prompt. Set `0700` on `~/.agentmarket/`, `0600` on `keystore.enc`.
- [ ] **T-012** Implement `output/formatter.rs` — user-facing message formatting, zero-crypto language. Exception: `init` and `fund` show wallet address. Must mask raw RPC errors, transaction hashes, and gas amounts — surface only human-readable messages.
- [ ] **T-013** Implement `engine/identity.rs` — secp256k1 keypair generation via `txgate`, public key derivation, profile schema (ERC-8004 registration file format)
  - Depends on: T-011
- [ ] **T-014** Implement `commands/init.rs` — prompts (name, description, capabilities, pricing), keypair generation, write `keystore.enc`, `config.toml`, `profile.json`. Display wallet address + funding instructions on completion.
  - Depends on: T-010, T-011, T-012, T-013
- [ ] **T-015** Tests: config store, keystore round-trip, identity engine, init command

---

## Phase 2: Chain Client, IPFS, Registration & Discovery (Base Sepolia)

### Infrastructure

- [ ] **T-020** Implement `chain/client.rs` — Base L2 RPC via `alloy` through public provider (Alchemy, etc.), nonce tracking, ETH balance checks, event log queries via `eth_getLogs`, retry with escalating gas, 1-block confirmation wait
- [ ] **T-021** Implement `chain/signer.rs` — TxGate transaction signing, key loading from keystore
  - Depends on: T-011
- [ ] **T-022** Implement `chain/contracts.rs` — ABI bindings for ERC-8004 Identity Registry, Reputation Registry, Validation Registry, USDC
- [ ] **T-023** Implement `chain/types.rs` — on-chain type definitions (AgentId, Request, Response, RequestStatus)
- [ ] **T-024** Implement `ipfs/client.rs` — IPFS HTTP API (add, cat, pin) via `reqwest`, local node or remote gateway
- [ ] **T-025** Implement `ipfs/pin.rs` — pinning service integration (Pinata), configurable API key via `AGENTMARKET_IPFS_PIN_KEY`
- [ ] **T-026** Implement `ipfs/encryption.rs` — ECIES encryption/decryption over secp256k1
- [ ] **T-027** Implement `ipfs/mailbox.rs` — encrypted mailbox: topic from `keccak256(public_key)`, publish/poll/decrypt
  - Depends on: T-024, T-026

### Commands

- [ ] **T-030** Implement `commands/fund.rs` — display wallet address, check ETH balance via RPC, report whether agent is ready to register
  - Depends on: T-010, T-011, T-012, T-020
- [ ] **T-031** Implement `commands/register.rs` — check ETH balance (fail with funding instructions if insufficient), validate `profile.json`, pin to IPFS, call ERC-8004 `register(agentURI)`, store `agentId` + CID in config, subscribe to mailbox
  - Depends on: T-013, T-020, T-021, T-022, T-024, T-025, T-027, T-012
- [ ] **T-032** Implement `commands/search.rs` — query contract event logs via `eth_getLogs` for registered agents and open requests, `--capability` filter, `--requests` flag, human-friendly output
  - Depends on: T-020, T-022, T-012

### Tests

- [ ] **T-035** Tests: chain client (mock RPC, balance checks, event log queries, retry), signer, IPFS client, ECIES round-trip, mailbox, register command, search command

---

## Phase 3: Transaction Lifecycle (Base Sepolia)

### Contract

- [ ] **T-040** Write `contracts/RequestRegistry.sol` — `createRequest()`, `submitResponse()`, `claim()`, `cancel()`, `expire()`, state structs, events
- [ ] **T-041** Write Foundry test suite for Request Registry
- [ ] **T-042** Deploy Request Registry to Base Sepolia
- [ ] **T-043** Add Request Registry ABI bindings to `chain/contracts.rs`
  - Depends on: T-022, T-040

### Engine & Commands

- [ ] **T-045** Implement `engine/requests.rs` — request lifecycle state machine, local cache in `~/.agentmarket/requests/`
- [ ] **T-046** Implement `commands/request.rs` — check ETH balance (fail with funding instructions if insufficient), build request JSON, encrypt payload (ECIES), pin to IPFS, call `createRequest()` + `USDC.approve()`. Flags: `--to`, `--task`, `--file`, `--price`, `--deadline`
  - Depends on: T-020, T-021, T-043, T-024, T-026, T-045, T-012
- [ ] **T-047** Implement `commands/respond.rs` — check ETH balance, retrieve/decrypt request, generate secret S, encrypt deliverable with S, pin to IPFS, call `submitResponse(requestId, ipfsCid, keccak256(S))`, store S locally
  - Depends on: T-020, T-021, T-043, T-024, T-026, T-045, T-012
- [ ] **T-048** Implement `commands/claim.rs` — check ETH balance, check validation status via event logs, read secret S from local storage, call `claim(requestId, S)`, update local earnings
  - Depends on: T-020, T-021, T-043, T-045, T-012
- [ ] **T-049** Implement `engine/reputation.rs` — query ERC-8004 Reputation Registry via event logs for on-chain trust scores
  - Depends on: T-020, T-022
- [ ] **T-050** Implement `commands/status.rs` — local earnings, USDC balance, ETH balance, reputation from event logs
  - Depends on: T-020, T-022, T-012, T-049

### Tests

- [ ] **T-051** Tests: request engine state machine, reputation engine, integration tests with Anvil + local IPFS
- [ ] **T-052** E2E test: request → respond on Sepolia (claim requires validation, tested in Phase 4)

---

## Phase 4: Validation & Daemon

### Validation

- [ ] **T-055** Implement `engine/validation.rs` — retrieve deliverable from IPFS, decrypt, invoke handler, determine pass/fail + score, route attestation to chain
  - Depends on: T-024, T-026, T-020, T-022
- [ ] **T-056** Implement handler invocation — spawn external process, pass deliverable on stdin, set `AGENTMARKET_*` env vars (`AGENTMARKET_REQUEST_ID`, `AGENTMARKET_TASK_TYPE`, `AGENTMARKET_SELLER`, `AGENTMARKET_DEADLINE`), parse exit code + JSON stdout (`{"score": N, "reason": "..."}`), configurable timeout (default 60s), kill process on timeout
- [ ] **T-057** Implement built-in `manual` handler — present deliverable to operator, prompt for approval
- [ ] **T-058** Implement `commands/validate.rs` — poll for pending validations via `eth_getLogs`, run validation loop, call `requestValidation()` + `submitValidation()`, track earnings. Flags: `--auto`, `--filter`, `--handler`
  - Depends on: T-055, T-056, T-057, T-020, T-043, T-012
- [ ] **T-059** Implement `commands/daemon.rs` — combine validate + auto-claim in a continuous loop, configurable poll interval, `SIGTERM` graceful shutdown
  - Depends on: T-058, T-048

### Tests

- [ ] **T-060** Tests: validation engine, handler invocation, daemon lifecycle
- [ ] **T-061** E2E test: full loop init → register → request → respond → validate → claim on Sepolia

---

## Phase 5: Mainnet & Ship

- [ ] **T-070** Implement `commands/withdraw.rs` — prompt for destination address, check ETH balance, call `USDC.transfer(destination, amount)`, confirm
  - Depends on: T-020, T-021, T-022, T-012
- [ ] **T-072** Switch default RPC to Base mainnet, deploy Request Registry to mainnet
- [ ] **T-073** Error handling pass — audit all commands and ensure human-readable errors for: insufficient ETH balance (show address + amount needed), insufficient USDC balance, RPC timeout / unreachable, IPFS node unreachable / pin failure, wrong keystore passphrase, expired request (can't claim), handler crash / timeout, missing config file, missing keystore, already registered, request not found. No raw RPC errors, tx hashes, or gas amounts in output.
- [ ] **T-074** Cross-compilation with `cross`, GitHub Releases with pre-built binaries, `cargo publish`
- [ ] **T-075** Write README.md

---

## Summary

| Phase | Tasks | Delivers |
|-------|:-----:|----------|
| 0 — Scaffolding | 4 | Compiling project, stubbed CLI |
| 1 — Identity | 6 | `init` works offline |
| 2 — Registration | 11 | `fund`, `register`, `search` on Sepolia |
| 3 — Transactions | 12 | `request`, `respond`, `claim`, `status`, reputation engine on Sepolia |
| 4 — Validation | 7 | `validate`, `daemon`, full E2E |
| 5 — Ship | 5 | `withdraw`, mainnet, error handling, release |
| **Total** | **45** | |

---

## Deferred (post-MVP)

- The Graph subgraph (replace `eth_getLogs` when query complexity demands it)
- OS keychain integration (macOS Keychain, Linux Secret Service)
- Built-in handlers: `compile-check`, `test-runner`, `llm-review`
- Daemon metrics endpoint (Prometheus)
- Daemon PID file
- ENS resolution for withdraw addresses
- IPFS attestation logs (off-chain reputation)
- Request cancellation CLI command
- Profile update after registration
- Key rotation / recovery
- Homebrew formula
- Docker image
- Encrypt secret S at rest
- Forward secrecy in ECIES mailbox
- Gas abstraction (Coinbase Paymaster or similar)
- Membership/access control (Unlock Protocol or similar)
