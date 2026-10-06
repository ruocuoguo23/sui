<!-- Copyright (c) Mysten Labs, Inc. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Dagora v1 validation

Validated on 2026-10-05 against Sui `mainnet-v1.80.1`, commit
`782577ca40c9ef0b3f400ade8bc2cc2dcd0e2b23`, on branch `codex/dag-abci-v1`.

| Check | Actual result |
| --- | --- |
| Build both binaries with `cargo build --locked -p dagora-node -p dagora-abci-mock` | Passed |
| `SUI_SKIP_SIMTESTS=1 cargo nextest run --locked -p dagora-abci -p dagora-abci-mock -p dagora-core -p dagora-node` | 29 passed |
| `SUI_SKIP_SIMTESTS=1 cargo nextest run -p consensus-core --lib` | 344 passed; one existing ignored test skipped |
| `cargo simtest -p consensus-simtests` | 12 passed, seed 1 |
| `python3 scripts/dagora/test_cluster.py` | Two real TCP regression tests passed |
| `python3 scripts/dagora/integration.py --duration 600` | All fault scenarios and the 600-second soak passed |
| Whole cluster stop and restart using its preserved data | Advanced beyond previous maximum height 5994 to common height 5998 |
| Rust formatting, Python/shell syntax, documentation source links, `git diff --check` | Passed |
| Normal runtime dependency tree | No `sui-core`, `sui-types`, `sui-execution`, or Move crates |

The real-process integration exercised empty blocks, successful and failed
transactions with identical receipts on all validators, progress with one
validator offline, both consensus and execution stopping after losing quorum,
node catchup, application loss and restart, and recovery after application Commit
persistence without its RPC acknowledgement. Journal unit tests cover input,
FinalizeBlock, result persistence, Commit, and cursor crash windows, inconsistent
replay, missing records, and mismatched hashes.

The soak reached common height **5954**. All four persisted block inputs and
execution results at that height were compared again after the run:

- Block hash: `b31bdcbdd1193796a928672a68f09794fe309274f07fbe9c256ddcc25371dc6e`
- Application hash: `2b9b82265f039cb9127af8611825e88631341b3deeeb6e0c9d5e1ebeb03b8d2d`
- Transaction count: zero, confirming empty blocks advance the application.

Local evidence remains in `/private/tmp/dagora-acceptance-soak-782577ca`,
`/private/tmp/dagora-acceptance-soak-782577ca.log`,
`/private/tmp/dagora-cluster-resume.log`,
`/private/tmp/dagora-crate-tests-locked.log`, and
`/private/tmp/dagora-consensus-simtests.log`. The owned cluster processes were
stopped after verification.

The simulator used an isolated Cargo cache and target directory. A temporary
copy of the repository's `cargo-simtest` wrapper omitted its unconditional
`git checkout -- Cargo.lock` cleanup, preserving the new Dagora dependencies;
the simulator flags, patches, profile, and test command were unchanged. The
workspace's original `futures-timer` version was restored afterwards, and the
final build and Dagora tests passed with `--locked`.

Protocol checks include unmodified upstream-source checksums, the official
service/method descriptors, and independently encoded protoc fixtures. A real
Cosmos SDK application, checkpoint certification, and epoch transitions have
not been integrated or verified by this version.


## Repository pruning validation — 2026-10-05

The [pruning report](PRUNING.md) and [explicit removal manifest](../../scripts/dagora/pruning-manifest.json)
describe the reduced workspace. This validation reruns the accepted v1 against
the pruned repository; the initial validation above is historical evidence.

| Actual command/check | Result |
| --- | --- |
| `cargo metadata --locked --offline --filter-platform aarch64-apple-darwin --format-version 1` | 22 workspace/local packages, no removed product packages |
| `RUSTFLAGS='--cfg msim' cargo tree --locked --offline --workspace --edges normal,build,dev` | 22 local packages, no Move, Sui execution/framework/SDK/RPC/core/types/config packages |
| Compare ordinary Cargo.lock package identities with pre-pruning lock | 1,384 → 607 external packages; no new versions or Git revisions |
| Verify archive bytes against pinned Git objects, then SHA-256 manifest again after pruning | All 571 original files match; 8,289,202 bytes |
| `cargo build --locked --offline -p dagora-node -p dagora-abci-mock` | Passed, including final build after simulator lock restoration |
| `SUI_SKIP_SIMTESTS=1 cargo nextest run --locked --offline -p dagora-abci -p dagora-abci-mock -p dagora-core -p dagora-node -p consensus-config -p consensus-core -p consensus-types -p consensus-simtests --lib` | 374 passed; 1 existing ignored consensus test skipped |
| `SUI_SKIP_SIMTESTS=1 cargo nextest run --locked --offline -p dagora-abci-mock -p dagora-node` | 8 binary tests passed |
| `cargo nextest run --locked --offline -p dagora-abci` | 3 schema/wire integration tests passed |
| `MSIM_TEST_SEED=1 cargo simtest --offline -p consensus-simtests` with repository runner | 15 passed: all 12 original scenarios and 3 address allocator tests |
| Compare lock bytes before/after actual simulator run | Exact current ordinary Cargo.lock restored |
| `python3 -m unittest discover -s scripts/simtest -p 'test_*.py' -v` | 14 passed: 9 fake-Cargo runner and 5 determinism-script regression cases |
| `python3 -m unittest discover -s scripts/dagora -p 'test_*.py' -v` | 2 real TCP regression tests passed |
| `CARGO_TARGET_DIR=/private/tmp/dagora-simtest-target MSIM_TEST_SEED=1 scripts/simtest/check-determinism.sh` | Actual consensus startup scenario passed twice with identical normalized output |
| `python3 scripts/dagora/integration.py --duration 600 --dir /private/tmp/dagora-pruning-acceptance-782577ca` | Fault cases, 600-second soak and same-data whole-cluster recovery passed |
| Four persisted inputs/results at common height 5938 | Equal across all validators; empty application block |
| Owned process cleanup | Control socket absent; all 12 test TCP ports available again |
| `cargo fmt --all --check`, Python compileall, retained shell syntax, Ruby YAML parsing, local guide/source links, `git diff --check` | Passed |
| External plan knowledge-base health check | 0 errors; 2 existing warnings |

The ordinary Rust checks cover **385 tests** in total, including all 29 Dagora
cases, 344 consensus-core cases, 9 consensus-config cases and 3 new address
allocator cases. The original ignored consensus test was not executed.

The new soak reached common height **5938**; whole-cluster restart advanced to
common height **5984** using the same keys and data. Four persisted records
were compared again after process cleanup:

- Block hash: `f8f3d9cc640cc0470c1bbfcaa9997e70c1f3b5b974c285f8b33f2e641216c4b8`.
- Application hash: `e804a25edde3f0769cca5a3bee79e72ece58d8ddea114d6f411da6c21f529b81`.
- Transaction count: 0.

The actual simulator uses the original pinned patches, seed 1, default watchdog
and separate target/cache directories. The restored lock is the current reduced
working-tree lock, rather than Git HEAD. The dependency-tree command above uses
cfg(msim) without temporary Tokio/futures-timer patches; those patches are exercised
by the actual 15-test simulator run.

Evidence is retained in `/private/tmp/dagora-pruning-acceptance-782577ca`,
`/private/tmp/dagora-pruning-acceptance.log`, `/private/tmp/dagora-pruning-summary.json`,
`/private/tmp/dagora-pruning-unit-tests.log`, `/private/tmp/dagora-pruning-binary-tests.log`,
`/private/tmp/dagora-pruning-abci-tests.log`, `/private/tmp/dagora-pruning-simtests.log`,
`/private/tmp/dagora-pruning-runner-tests.log`, and `/private/tmp/dagora-pruning-determinism.log`.

Hosted Linux GitHub jobs are configured and syntax-checked, but were not executed
from this session. Full support-package unit/doc tests and clippy were not run.
Real Cosmos SDK execution, checkpoint certification, epoch rotation and state
sync remain outside this validation.


## Commit preparation — 2026-10-06

The complete runtime validation above remains dated 2026-10-05. Before creating
the local commit, archive bytes were compared again with all 571 pinned Git
objects; package/deletion scope and available test evidence were independently
reviewed. No runtime behavior changed during commit preparation.

Fresh checks passed: `cargo fmt --all --check`; 14 simulator-script regression
cases; 2 real TCP supervisor regression cases; all 3 `dagora-abci` nextest
schema/wire tests; Python/shell syntax; YAML parsing; local guide links; and
`git diff --cached --check --find-renames=100%`. Six extra EOF blank lines in new
ABCI manifest/docs/fixture inputs were removed; all five independently encoded
protoc fixtures still match their stored bytes. Archived source bytes were left
unchanged. The 600-second soak and full consensus suites were not repeated for
these documentation and whitespace changes.
