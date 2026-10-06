<!-- Copyright (c) Mysten Labs, Inc. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Dagora core

`dagora-core` consumes finalized Mysticeti commits, derives application blocks,
and coordinates execution through `BlockExecutor`. DAG proposal, commit selection,
linearization, and consensus persistence remain in the upstream `consensus` crates.
`dagora-node` connects this execution interface to a separate ABCI application.

## Block and progress model

Every finalized `CommittedSubDag` produces one `ChainBlock`, including commits
with no transactions or only rejected transactions. DAG blocks are ordered by
`(round, author, digest)`; transactions retain their order within each DAG block.
Rejected transaction indices are zero-based and excluded from application input.
Commit timestamps come from consensus, can repeat, and never use the local
execution clock. Recovery/local-decision flags do not enter canonical block data.

The progress identifiers are separate:

- `ConsensusPosition { epoch, index, digest }` identifies an epoch-local commit.
- `ChainBlock.height` is the application chain's global height, allocated from
  the durable execution cursor plus one.

The fixed epoch-0 deployment naturally starts both sequences at one. Code checks
their continuity separately and replay looks up `(epoch, index)`, then verifies
the complete recorded block. Global height must not become an alias for commit
index when epoch rotation is added.

`block_hash` is lowercase hexadecimal SHA256 of the BCS tuple
`(chain_id, position, height, timestamp_ms, previous_block_hash, transactions)`.
It excludes the `block_hash` field itself. Genesis uses 32 zero bytes for the
previous block hash and application hash, with application height zero.

Submission acknowledgement means proposal inclusion. Application completion
requires `FinalizeBlock`, `Commit`, matching `Info`, and a durable cursor update.
Only then may the caller advance `CommitConsumerMonitor`.

## Durable execution and recovery

`Journal::open` takes an OS-backed exclusive directory lock. In its `journal/`
directory, each height has immutable `N.input.json` and `N.result.json` files;
`cursor.json` is replaced atomically. Writes synchronize the file and directory.
Input is durable before `FinalizeBlock`; its complete execution result is durable
before `Commit`. The cursor advances only after `Info` confirms height and hash.
Public block/receipt getters expose completed heights; `pending_height` identifies
unapplied input. No records are pruned.

Here `H` is the durable cursor height:

| Durable journal | Application `Info` | Recovery |
| --- | --- | --- |
| No pending input | Height H, matching hash | Resume after the cursor's consensus position. |
| Input H+1, no result | Height H, matching hash | Finalize recorded input, persist result, then Commit and verify Info. |
| Input and result H+1 | Height H, matching hash | Replay FinalizeBlock; require identical result before Commit. |
| Input and result H+1 | Height H+1, matching result hash | Advance cursor without repeating FinalizeBlock or Commit. |
| Pending Commit, Info unavailable | Unknown | Preserve pending files and fail; resolve through Info after restart. |
| Missing input/result, gap, or mismatched block/app hash | Any inconsistent state | Fail without advancing or resetting state. |

A lost Commit acknowledgement is resolved with `Info`, rather than assuming the
application rolled back. If Info still reports H, the pending result remains for
recovery. At genesis, `InitChain` must be idempotent for the same chain ID and
reject a different chain ID; this also checks binding after a genesis restart.

Journal loading checks record presence, heights and positions, canonical block
hashes, result count/hash length, and cursor consistency. Recovery compares the
application hash and deterministic replay result. Receipt files remain trusted
local storage: these checks are not cryptographic receipt certification or a
signed checkpoint protocol.

## Current boundary and later Sui capability ports

V1 has a fixed epoch-0 committee, v2 consensus configuration, and opaque-byte
validation. Transaction voting and slim propagation are disabled. There is no
checkpoint certification, dynamic epoch transition, or application state sync.
`BlockExecutor` is the execution boundary; business state stays in the application.

Future ports can use the [pinned Sui reference archive](../../upstream/sui/README.md).
It preserves upstream paths and source bytes outside the supported workspace;
its original manifests and tests are not a build target. Relevant sources include:

- [ConsensusManager](../../upstream/sui/crates/sui-core/src/consensus_manager/mod.rs): `start`/`shutdown`
  coordinate clients, commit consumers, replay cursors, and per-epoch consensus DBs.
- [Consensus handler](../../upstream/sui/crates/sui-core/src/consensus_handler.rs):
  `MysticetiConsensusHandler` consumes ordered commits and reports progress.
  [ConsensusCommitAPI](../../upstream/sui/crates/sui-core/src/consensus_types/consensus_output_api.rs)
  adapts output to Sui transaction types; replace that application-specific part.
- [Checkpoints](../../upstream/sui/crates/sui-core/src/checkpoints/mod.rs): `CheckpointBuilder` and
  `CheckpointService` build and certify checkpoints.
  [CheckpointExecutor](../../upstream/sui/crates/sui-core/src/checkpoints/checkpoint_executor/mod.rs)
  uses `run_epoch` to execute an epoch through its final checkpoint.
- [Epoch state](../../upstream/sui/crates/sui-core/src/epoch/reconfiguration.rs) and
  [CommitteeStore](../../upstream/sui/crates/sui-core/src/epoch/committee_store.rs) illustrate admission
  closure and committee history. [EpochStartConfiguration](../../upstream/sui/crates/sui-core/src/authority/epoch_start_configuration.rs)
  contains Sui system-object state and should be replaced with Dagora types.
- [Sui node](../../upstream/sui/crates/sui-node/src/lib.rs): `monitor_reconfiguration` and
  `reconfigure_state` coordinate the executed final checkpoint and next epoch;
  [AuthorityState::reconfigure](../../upstream/sui/crates/sui-core/src/authority.rs) handles Sui execution state.

Port application commit/checkpoint/epoch coordination into `dagora-core` without
duplicating Mysticeti's commit algorithm or adding a runtime `sui-core` dependency.
These are source references, not implemented migration features.
Sui effects, object causal ordering, and parallel checkpoint execution stay
outside the ABCI execution driver.

Core tests: `SUI_SKIP_SIMTESTS=1 cargo test -p dagora-core`.
