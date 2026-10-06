# Consensus simulator tools

Run from the repository root with Rust and `cargo-nextest` installed:

```bash
scripts/simtest/install.sh
MSIM_TEST_SEED=1 cargo simtest -p consensus-simtests
# Direct invocation also works without installation:
scripts/simtest/cargo-simtest simtest build -p consensus-simtests
```

The runner applies the pinned msim Tokio and futures-timer patches, enables
`--cfg msim`, and defaults to seed `1` and a 60-second watchdog. Existing seed,
watchdog, and `RUSTFLAGS` values are preserved. `LOCAL_MSIM_PATH` selects a local
msim checkout; `USE_MOCK_CRYPTO=1` enables the existing mock-crypto patch.

Before invoking Cargo, the runner saves the current working-tree `Cargo.lock`,
including uncommitted changes. It restores the exact bytes on success, failure,
and catchable `HUP`, `INT`, or `TERM`, preserving the command's exit code. Signal
handling waits for its Cargo child to stop before restoring. If restoration
fails, the runner exits nonzero and prints the retained backup path. `SIGKILL`
cannot run cleanup; do not run concurrent Cargo commands against the same
workspace while the simulator runner is active.

Search multiple seeds, running each test and seed in its own process:

```bash
python3 scripts/simtest/seed-search.py \
  --package consensus-simtests --num-seeds 20 --seed-start 1 \
  --concurrency 4 --no-reachability --log-dir /tmp/dagora-sim-seeds
```

`failures.ndjson` and per-test logs in the selected directory record failures.
The optional reachability helper uses macOS Mach-O tools; use `--no-reachability`
on Linux. After building the simulator tests, compare the consensus startup
scenario's output across two independent processes:

```bash
MSIM_TEST_SEED=1 scripts/simtest/check-determinism.sh
```

The determinism checker honors `CARGO_TARGET_DIR` and checks the already-built
`consensus_tests::test_committee_start_simple` test. It rejects failed runs and
silently missing tests as well as differing output.

The script regression suite uses fake Cargo and test executables in temporary
directories, without building Rust or changing the repository lockfile:

```bash
python3 -m unittest discover -s scripts/simtest -p 'test_*.py' -v
```
