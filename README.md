<!-- Copyright (c) Mysten Labs, Inc. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Dagora

Dagora is a small chain built on Sui's Mysticeti DAG consensus. Four equal-weight
validators order opaque transaction bytes and send each finalized consensus
commit to a separate Rust ABCI application. Empty commits also produce application
blocks. The application owns execution and state; the node persists blocks,
receipts, and an execution cursor for restart recovery.

The consensus baseline is Sui `mainnet-v1.80.1`, commit
`782577ca40c9ef0b3f400ade8bc2cc2dcd0e2b23`. The active Rust workspace contains
22 packages: four consensus packages, four Dagora packages, and fourteen
support packages for storage, networking, cryptography, metrics, macros, and
simulation. Sui product applications, SDKs, Move execution, indexers, and their
build tooling are outside this workspace.

| Path | Purpose |
| --- | --- |
| `consensus/` | Mysticeti DAG, commit ordering, consensus storage, and simulation tests |
| `crates/dagora-core/` | Deterministic application blocks, durable execution journal, executor boundary |
| `crates/dagora-node/` | Validator binary, ABCI client, local transaction/status/block HTTP API |
| `crates/dagora-abci/` | Official CometBFT v0.38.22 protobuf schemas and shared Rust bindings |
| `crates/dagora-abci-mock/` | Separate deterministic Rust application with durable state |
| `scripts/dagora/` | Local cluster supervisor and acceptance checks |
| `upstream/sui/` | Source reference archive for future checkpoint, epoch, and state sync work |

The [reference archive](upstream/sui/README.md) preserves original paths and
copyright notices. Its [manifest](upstream/sui/manifest.json) records source
paths, byte lengths, and SHA-256 hashes. It is retained for reading and later
ports; its original Sui manifests and tests are not a supported build target.

## Run a local network

Use the Rust version pinned in [rust-toolchain.toml](rust-toolchain.toml).
Building requires `protoc`, a C/C++ toolchain, CMake, libclang, and OpenSSL
development files. On Ubuntu:

```bash
sudo apt-get update
sudo apt-get install -y build-essential clang libclang-dev cmake protobuf-compiler pkg-config libssl-dev
cargo build --locked -p dagora-node -p dagora-abci-mock
bash scripts/dagora/start.sh --dir /tmp/dagora-local --port-base 26000
target/debug/dagora-node status --url http://127.0.0.1:26010
target/debug/dagora-node submit --url http://127.0.0.1:26010 --tx 'hello'
target/debug/dagora-node submit --url http://127.0.0.1:26010 --tx 'fail:example'
target/debug/dagora-node block --url http://127.0.0.1:26010 --height 1
bash scripts/dagora/stop.sh --dir /tmp/dagora-local
```

Starting with the same directory resumes existing keys, consensus storage,
execution journals, and application state. Without `--port-base`, the starter
chooses available ports and prints each HTTP URL. For a custom Cargo target
directory, pass `--bin-dir PATH` to the scripts.

The mock accepts `fail:` transactions at admission and returns execution code 1
while committing the block. Other valid transactions return code 0. Its app hash
is `SHA256(previous_app_hash || block_hash)`, starting from 32 zero bytes.
Submission acknowledges proposal inclusion; inspect the persisted block receipt
to confirm execution.

See the [pruning report](crates/dagora-node/PRUNING.md),
[validation record](crates/dagora-node/VALIDATION.md),
the [node guide](crates/dagora-node/README.md),
[durability and recovery model](crates/dagora-core/README.md), and
[ABCI schema provenance](crates/dagora-abci/UPSTREAM.md).

## Check changes

```bash
cargo fmt --all -- --check
cargo install cargo-nextest --locked
SUI_SKIP_SIMTESTS=1 cargo nextest run --locked \
  -p dagora-abci -p dagora-abci-mock -p dagora-core -p dagora-node \
  -p consensus-config -p consensus-types -p consensus-core
SUI_SKIP_SIMTESTS=1 cargo nextest run --locked -p consensus-simtests --lib
python3 scripts/dagora/test_cluster.py
python3 -m unittest discover -s scripts/simtest -p 'test_*.py' -v
export PATH="$PWD/scripts/simtest:$PATH"
MSIM_TEST_SEED=1 MSIM_WATCHDOG_TIMEOUT_MS=300000 \
  cargo simtest -p consensus-simtests --profile ci --test-threads 1
python3 scripts/dagora/integration.py --duration 600
```

Run simulator tests through `cargo simtest`; ordinary test execution does not
exercise their simulated network. The simulator runner applies pinned dependency
patches, backs up and restores the workspace lockfile, and uses a fresh runtime
temporary directory. Run Cargo commands sharing this workspace serially; no
other Cargo writer may run while the simulator runner owns the lockfile.
The consensus suite includes twelve integration simulation scenarios.

The acceptance script checks matching blocks and receipts, successful and
failed transactions, progress with one validator offline, a halt with two
offline, node/application restart recovery, and recovery when the application
persists Commit but loses its reply. It then checks ten minutes of progress and
agreement and resumes the whole cluster using the same data directory.
Use `--duration 0` to omit the soak. GitHub CI runs these checks on hosted Ubuntu.

## Current limits

V1 fixes the committee at epoch 0. It has no signed checkpoint service, committee
rotation, or application state sync. HTTP ingress is local and unauthenticated;
payloads are nonempty UTF-8 bytes up to 256 KiB. Transaction voting and slim
propagation are disabled. Application updates to validators or consensus
parameters stop the node with an error.

The wire boundary uses CometBFT v0.38.22 ABCI gRPC, with `Info`, `InitChain`,
`CheckTx`, `FinalizeBlock`, and `Commit`. The mock does not run Cosmos SDK or a
second consensus process. A real Cosmos SDK application has not been validated;
Tendermint block hashes, light clients, vote extensions, and snapshots require
additional integration work. Local execution receipts are not certified
checkpoints.

Contributions follow [CONTRIBUTING.md](CONTRIBUTING.md). Code is licensed under
[Apache-2.0](LICENSE); archived documentation retains its [documentation license](upstream/sui/LICENSE-docs).
