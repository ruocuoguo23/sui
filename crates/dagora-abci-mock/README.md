# Dagora ABCI mock

This Rust binary exposes the official CometBFT v0.38.22 gRPC ABCI service. It is
a protocol-compatible test application, with no Cosmos SDK or CometBFT
consensus process.

```sh
cargo run -p dagora-abci-mock -- --listen 127.0.0.1:26658 --data-dir /tmp/dagora-app-0
```

`InitChain` binds a nonempty chain ID at initial height 1 and starts with a
32-byte zero app hash. The same initialization can be retried at height zero,
including while the first block is pending, without discarding its execution.
`FinalizeBlock` stages deterministic results; `Commit` atomically writes and
fsyncs `state.json` before replying. Pending execution disappears on restart.
An exclusive data directory lock prevents two processes from using one state.

- `CheckTx` accepts opaque payloads up to 256 KiB. Larger payloads return code 1.
- Transactions beginning with `fail:` pass admission and fail execution with
  code 1. Other transactions succeed. Every ordered transaction receives a result.
- App hash is `SHA256(previous_app_hash[32] || block_hash[32])`. The block hash
  must already commit to the ordered block input. This mock does not provide
  application-state Merkle proofs.
- `Info` reports only the last committed height and hash. Repeated `Commit`
  without pending execution is harmless. Gaps, old blocks and conflicting
  pending inputs fail explicitly.

Use `--fail-after-commit-once` to exit with code 86 after a durable block commit
but before sending its response. A persisted marker prevents repeating that
fault after restart with the same flag and directory. This exercises the
executor's uncertain-Commit recovery through `Info`.

The five implemented RPCs are `Info`, `InitChain`, `CheckTx`, `FinalizeBlock`
and `Commit`. Other official methods return `UNIMPLEMENTED`; validator changes,
proposal hooks, snapshots and vote extensions are outside the v1 mock.
