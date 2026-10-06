# Repository instructions

Consult crate-specific `CLAUDE.md` or `AGENTS.md` files before changing a crate.
Crate instructions override these defaults. Respect any `CLAUDE.local.md`
preferences when present.

## Scope and provenance

This is the 22-package Dagora Rust workspace based on Sui
`mainnet-v1.80.1` at `782577ca40c9ef0b3f400ade8bc2cc2dcd0e2b23`.

- `consensus/` owns the upstream Mysticeti DAG, commit algorithm, storage, and simulations.
- `dagora-core` derives deterministic application blocks and coordinates durable execution.
- `dagora-node` connects consensus and the execution journal to an external ABCI application.
- `dagora-abci` owns the official CometBFT v0.38.22 schema and shared wire bindings.
- `dagora-abci-mock` is a separate Rust process with deterministic execution and durable state.
- Remaining workspace packages support storage, networking, crypto, metrics, macros, and simulation.

[upstream/sui](upstream/sui/README.md) is a source reference archive for future
checkpoint, epoch, and state sync ports. It is excluded from the workspace and
is not maintained as a buildable Sui distribution. Preserve its original bytes,
paths, license notices, and provenance manifest; port changes into active Dagora
crates rather than modifying archived sources.

V1 uses a fixed four-validator epoch-0 committee. There is no Move execution,
Sui object/effects model, checkpoint certification, validator rotation, or
application state sync. Do not introduce these as incidental dependencies.

## License comments

Applicable source files start with:

```text
Copyright (c) Mysten Labs, Inc.
SPDX-License-Identifier: Apache-2.0
```

Vendored third-party schemas and archived sources preserve their original
copyright and licensing.

## Build and test

Use the toolchain in `rust-toolchain.toml`. Install `protoc`, a C/C++ toolchain,
CMake, libclang, and OpenSSL development files for native dependencies.

```bash
cargo check --locked -p dagora-node
cargo build --locked -p dagora-node -p dagora-abci-mock
cargo fmt --all -- --check
cargo clippy --locked -p dagora-core -p dagora-node --all-targets -- -D warnings
cargo install cargo-nextest --locked
SUI_SKIP_SIMTESTS=1 cargo nextest run --locked \
  -p dagora-abci -p dagora-abci-mock -p dagora-core -p dagora-node \
  -p consensus-config -p consensus-types -p consensus-core
SUI_SKIP_SIMTESTS=1 cargo nextest run --locked -p consensus-simtests --lib
python3 scripts/dagora/test_cluster.py
python3 -m unittest discover -s scripts/simtest -p 'test_*.py' -v
```

Scope routine checks with `-p`; give builds and suites at least ten minutes
before treating them as timed out. Add meaningful tests for behavior changes and
crash/recovery boundaries. Use `#[tokio::test]` for async tests. Keep test-only
code under `#[cfg(test)]`; cross-crate test helpers use a propagated
`testing` feature.

Simulator tests must run with the patched simulator runner:

The runner backs up and restores the current workspace lockfile while applying
simulation dependency patches. Run Cargo writers sharing this workspace
serially; do not run other Cargo commands concurrently with the simulator.

```bash
export PATH="$PWD/scripts/simtest:$PATH"
MSIM_TEST_SEED=1 MSIM_WATCHDOG_TIMEOUT_MS=300000 \
  cargo simtest -p consensus-simtests --profile ci --test-threads 1
```

For changes across consensus, execution, or ABCI recovery, build both binaries
and run `python3 scripts/dagora/integration.py --duration 600`. This includes
same-data-directory recovery after the soak. Use `--duration 0` for focused
failure/restart checks during iteration.

Do not accept unrelated snapshot changes or claim planned checks have passed.
Report the commands actually run and any remaining coverage limits.

## Implementation rules

Keep consensus persistence ahead of output, journal input ahead of FinalizeBlock,
execution results ahead of Commit, and the execution cursor behind verified
application Info. Submission acknowledgement means proposal inclusion, not
successful execution. Advance the commit consumer watermark only after durable
application completion.

Keep canonical block data deterministic. Local clocks, recovery flags, and
process scheduling must not enter block hashes or consensus-visible execution
results. Business state belongs to the external application.

Keep changes scoped to the request. Comment non-obvious invariants and recovery
choices; avoid narrating obvious code. Release binaries inherit `panic=abort`.

When creating commits, use an English title and body covering Background,
Changes, actual Validation, and any Risks. When opening or updating a PR, use the
`send-pr` skill if available and follow its applicable instructions.
