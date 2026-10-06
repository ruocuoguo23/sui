<!-- Copyright (c) Mysten Labs, Inc. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Dagora v1 local network

Four equal validators use the existing Mysticeti DAG engine to order opaque
transaction bytes. Each validator connects to a separate ABCI gRPC application.
`dagora-core` persists deterministic chain blocks and execution receipts, and
recovers the application through `Info` before resuming consensus output.

See [core durability and future Sui capability ports](../dagora-core/README.md)
and [ABCI schema provenance and wire checks](../dagora-abci/UPSTREAM.md).
Building the protocol crate requires `protoc` on `PATH`.

```bash
cargo build -p dagora-node -p dagora-abci-mock
bash scripts/dagora/start.sh --dir /private/tmp/dagora-local
python3 scripts/dagora/cluster.py status --dir /private/tmp/dagora-local
```

The starter prints four HTTP URLs. Ports are reserved during initialization and
assigned independently from other local clusters. Supply `--port-base 26000` to
use consensus ports 26000–26003, HTTP ports 26010–26013, and ABCI ports
26020–26023. For custom Cargo target directories, pass `--bin-dir PATH` to the
scripts.

```bash
target/debug/dagora-node status --url http://127.0.0.1:26010
target/debug/dagora-node submit --url http://127.0.0.1:26010 --tx 'hello'
target/debug/dagora-node submit --url http://127.0.0.1:26010 --tx 'fail:example'
target/debug/dagora-node block --url http://127.0.0.1:26010 --height 1
bash scripts/dagora/stop.sh --dir /private/tmp/dagora-local
```

Use the URLs returned by the starter when a port base was not specified.
`fail:` is a mock execution fixture: `CheckTx` accepts it, `FinalizeBlock`
returns transaction code 1, and the block still commits. Other valid payloads
return code 0. The deterministic mock application hash is
`SHA256(previous_app_hash || block_hash)`, beginning at 32 zero bytes.

The supervisor owns its direct child processes and accepts control commands
over a private local socket. It locks its directory and stops only its own
children. Stopping a cluster preserves keys, consensus storage, block journals,
and application storage. Starting with the same directory resumes that chain.

## Single-validator commands and HTTP API

`dagora-node init --dir PATH` creates a public `committee.json` and four
`nodeN/config.json` files. Protocol and network keys are independently generated
from OS entropy and saved in owner-only key files. Initialization refuses to
overwrite existing files. For a manual launch, start each application with
`dagora-abci-mock --listen ADDRESS --data-dir PATH`, then start the matching
validator with `dagora-node run --config PATH/nodeN/config.json`.

| Endpoint | Result |
| --- | --- |
| `GET /status` | Chain ID, epoch, durable consensus commit index, executed commit index, execution height, app hash, block hash, and pending height |
| `GET /blocks/{height}` | Completed `block` and its persisted `execution` receipt |
| `POST /transactions` | JSON `{"tx":"UTF-8 payload"}`; returns transaction hash and proposed DAG block reference |

Submission success means the transaction entered a proposed DAG block. Observe
`/blocks` to confirm final inclusion and execution. A submission timeout leaves
its status unknown; automatically resubmitting could execute the same payload
again. The v1 ingress accepts nonempty payloads up to 256 KiB. `CheckTx` uses a
separate ABCI connection and does not participate in consensus verification.

Only the execution driver issues ordered `FinalizeBlock`/`Commit` calls. RPCs
have five-second deadlines. Losing the application or receiving an unsupported
validator/consensus-parameter update stops the validator with an error; it does
not advance the application execution watermark. Restart the application and
validator to recover their durable state. The mock's
`--fail-after-commit-once` fixture exits after persisting its first Commit but
before replying, allowing recovery of an uncertain Commit response to be tested.

## Verification

See the [recorded local acceptance results](VALIDATION.md).

```bash
SUI_SKIP_SIMTESTS=1 cargo test -p dagora-node
python3 scripts/dagora/test_cluster.py
python3 scripts/dagora/integration.py --duration 600
```

The integration script starts its own local cluster, compares common block and
receipt prefixes, checks successful and failed transactions, verifies progress
with one validator offline and a halt with two offline, restarts nodes, kills
and restarts an application, tests uncertain Commit recovery, and then checks
continued progress and agreement for ten minutes. Use `--duration 0` for the
functional failure/recovery scenarios without the soak. Logs and persisted data
remain in the printed directory after the owned cluster is stopped.

For manual fault checks, use `cluster.py stop-node --index N`,
`restart-node --index N`, `stop-app --index N`, or `restart-app --index N`, always
with the same `--dir`.

This version has a fixed four-validator epoch-0 committee and local-only
unauthenticated HTTP ingress. It has no signed checkpoint service, validator
rotation, Tendermint block or light-client compatibility, state sync, wallet
format, or production transaction validation. The ABCI protobuf boundary is
CometBFT v0.38; custom applications must accept the Dagora block hash and fixed
committee semantics described here.
Integration with a real Cosmos SDK application has not been verified.

Repository scope and retained Sui references: [pruning report](PRUNING.md).
