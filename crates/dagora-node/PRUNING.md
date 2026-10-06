<!-- Copyright (c) Mysten Labs, Inc. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Repository pruning

The active workspace is reduced from **149 to 22 Rust packages** on
`codex/dag-abci-v1`, preserving the existing Dagora implementation.
The base remains Sui `mainnet-v1.80.1` at
`782577ca40c9ef0b3f400ade8bc2cc2dcd0e2b23`.

## Retained code and references

The four consensus packages and four Dagora packages remain active, with fourteen
storage/network/crypto/metrics/macro/simulator support packages. The exact member
and removal lists are recorded in the [pruning manifest](../../scripts/dagora/pruning-manifest.json).

[upstream/sui](../../upstream/sui/README.md) contains **571 original files**
(**8,289,202 bytes**) verified against the release before removal of active Sui
product sources. It includes complete sui-core, sui-types, sui-node and original
sui-simulator sources, state sync implementation, selected checkpoint and epoch
integration tests, architecture documentation and the Lutris paper. The
[archive manifest](../../upstream/sui/manifest.json) records every original path,
SHA-256 digest and size; copyright notices and licenses are retained. The archived
manifests and tests are source references, outside the supported build.

112 unused active crate directories, the Move/compiler/VM tree, Sui execution,
bridge, SDK, dapps, examples, website, packaging and release infrastructure were
removed. 47 inherited GitHub files were replaced by hosted Ubuntu Dagora CI.
The cleanup used an explicit removal list without resetting the repository or
using git clean. Existing Dagora files, build artifacts, IDE state and local
test data were preserved.

## Build and simulator changes

Unused workspace declarations and overrides were removed. The ordinary lockfile
shrunk from 1,384 to 607 external package entries, with no surviving version or
Git revision upgrades. The 111 retained dependency declarations are used by the
active packages.

Consensus simulation scenarios retain their original test bodies and failpoints.
Only their deterministic address allocator dependency was moved into test
support. Three tests cover address uniqueness, UDP formatting and IP exhaustion.
The simulator retains msim exports, delay configuration, node/leak tracking and
random helpers, with rand small_rng explicitly enabled.

The simulator runner backs up the current lockfile before Cargo and restores
exact bytes and exit status after success, failure or catchable signals. It
retains the backup and fails if restoration fails. Cargo writers sharing the
workspace lockfile must run serially. SIGKILL cannot execute cleanup.

JS workspaces, Move formatting, removed-crate aliases and indexer nextest setup
were removed. README, contribution instructions, simulator guides and archive
links describe the reduced project. Future checkpoint certification, epoch
rotation, state sync and real Cosmos SDK integration remain separate work.

## Validation

Actual commands and outcomes are recorded in [VALIDATION.md](VALIDATION.md).
The new implementation plan is saved in the user-specified external plan directory.
