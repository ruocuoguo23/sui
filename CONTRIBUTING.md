<!-- Copyright (c) Mysten Labs, Inc. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Contributing to Dagora

Keep changes focused on the consensus-to-application boundary. The active
workspace is described in [README.md](README.md); the
[Sui archive](upstream/sui/README.md) is preserved reference material, outside
the supported build. Retain upstream copyright and license notices.

## Development

Use the pinned Rust toolchain and install the native build dependencies listed
in the README. Create a `codex/` branch when working through Codex. For behavioral
changes, add meaningful tests covering normal behavior, key boundaries, and
failure paths. Recovery changes need tests around both node journal persistence
and application Commit persistence.

```bash
cargo fmt --all -- --check
cargo build --locked -p dagora-node -p dagora-abci-mock
cargo install cargo-nextest --locked
SUI_SKIP_SIMTESTS=1 cargo nextest run --locked \
  -p dagora-abci -p dagora-abci-mock -p dagora-core -p dagora-node \
  -p consensus-config -p consensus-types -p consensus-core
SUI_SKIP_SIMTESTS=1 cargo nextest run --locked -p consensus-simtests --lib
python3 scripts/dagora/test_cluster.py
python3 -m unittest discover -s scripts/simtest -p 'test_*.py' -v
```

Run `cargo simtest -p consensus-simtests --profile ci --test-threads 1`
through the patched runner for consensus simulation changes; installation and
environment setup are in the README. Run Cargo writers sharing one workspace
serially because the simulator runner temporarily patches and restores that
workspace's lockfile. Run
`python3 scripts/dagora/integration.py --duration 600` for changes spanning
consensus, execution, and ABCI recovery. Hosted Ubuntu CI runs the complete
acceptance suite, including restart with the same stored chain data.

Do not update archived sources to make a Dagora change. Port the necessary
behavior into active crates, keeping application-specific Sui types out of the
Dagora execution interface. When documentation references archived files,
link through `upstream/sui/`.

## Issues and pull requests

For bugs, include the revision, OS, Rust version, reproduction steps, relevant
logs, and expected/actual behavior. Include node and application heights when
reporting recovery failures. Remove private keys and credentials from reports.

Describe the problem and the resulting behavior in a PR. State the checks
actually performed and any missing coverage. Do not describe planned tests as
passed. Include compatibility, migration, or known limitations when relevant.

Commit messages use an English title and body with Background, Changes,
Validation, and Risks when applicable. Match the level of detail to the change.

By contributing, you agree to license your contribution under [Apache-2.0](LICENSE).
