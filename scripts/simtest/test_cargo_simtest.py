#!/usr/bin/env python3
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0

"""Exercise the runner with a fake Cargo; no Rust builds or real lockfile edits."""

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest


RUNNER = Path(__file__).with_name("cargo-simtest").resolve()
ORIGINAL_LOCK = b"# current working tree\nversion = 4\n\n# preserved bytes: \xff\x00\n"
FAKE_CARGO = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import signal
import sys
import time

workspace = Path(os.environ["FAKE_WORKSPACE"])
args = sys.argv[1:]
with (workspace / "calls.jsonl").open("a") as output:
    output.write(json.dumps({"args": args, "env": dict(os.environ)}) + "\n")
backups = list((workspace / "tmp").glob("cargo-simtest-lock.*/Cargo.lock"))
if len(backups) != 1 or backups[0].read_bytes().hex() != os.environ["ORIGINAL_LOCK_HEX"]:
    sys.exit(98)
lock = workspace / "Cargo.lock"
lock.write_bytes(b"lock changed by Cargo\n")
if args == ["nextest", "--help"]:
    sys.exit(int(os.environ.get("FAKE_HELP_EXIT", "0")))
mode = os.environ.get("FAKE_MODE", "exit")
if mode == "signal":
    def stop(signum, _frame):
        lock.write_bytes(b"last Cargo write before signal exit\n")
        (workspace / "child-stopped").write_text(str(signum))
        sys.exit(128 + signum)
    for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
        signal.signal(signum, stop)
    (workspace / "ready").write_text(str(os.getpid()))
    while True:
        time.sleep(0.05)
elif mode == "restore-failure":
    lock.unlink()
    lock.mkdir()
elif mode == "delete-lock":
    lock.unlink()
sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
'''


class CargoSimtestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="simtest-runner-test-")
        self.addCleanup(self.temporary.cleanup)
        self.workspace = Path(self.temporary.name)
        self.lock = self.workspace / "Cargo.lock"
        self.lock.write_bytes(ORIGINAL_LOCK)
        (self.workspace / "Cargo.toml").write_text("[workspace]\n")
        (self.workspace / "tmp").mkdir()
        self.child_dir = self.workspace / "nested"
        self.child_dir.mkdir()
        binaries = self.workspace / "bin"
        binaries.mkdir()
        fake_cargo = binaries / "cargo"
        fake_cargo.write_text(FAKE_CARGO)
        fake_cargo.chmod(0o755)
        self.env = os.environ.copy()
        for name in ("LOCAL_MSIM_PATH", "USE_MOCK_CRYPTO", "MSIM_TEST_SEED", "MSIM_WATCHDOG_TIMEOUT_MS", "RUSTFLAGS"):
            self.env.pop(name, None)
        self.env.update({
            "PATH": str(binaries) + os.pathsep + self.env["PATH"],
            "TMPDIR": str(self.workspace / "tmp"),
            "FAKE_WORKSPACE": str(self.workspace),
            "ORIGINAL_LOCK_HEX": ORIGINAL_LOCK.hex(),
        })

    def run_runner(self, *args, **env):
        return subprocess.run(
            [str(RUNNER), "simtest", *args],
            cwd=self.child_dir,
            env=dict(self.env, **env),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
        )

    def assert_restored(self):
        self.assertEqual(self.lock.read_bytes(), ORIGINAL_LOCK)
        self.assertEqual(list((self.workspace / "tmp").glob("cargo-simtest-lock.*")), [])

    def calls(self):
        return [json.loads(line) for line in (self.workspace / "calls.jsonl").read_text().splitlines()]

    def test_success_preserves_current_lock_instead_of_head(self):
        git = shutil.which("git")
        if git is None:
            self.skipTest("git is required for the HEAD regression fixture")
        subprocess.run([git, "init", "--quiet", str(self.workspace)], check=True)
        self.lock.write_bytes(b"# committed HEAD lock\n")
        subprocess.run([git, "-C", str(self.workspace), "add", "Cargo.lock"], check=True)
        subprocess.run([
            git, "-C", str(self.workspace), "-c", "user.name=Runner Test",
            "-c", "user.email=runner@example.invalid", "commit", "--quiet", "-m", "Lock fixture",
        ], check=True)
        self.lock.write_bytes(ORIGINAL_LOCK)
        self.assertNotEqual(subprocess.check_output([git, "-C", str(self.workspace), "show", "HEAD:Cargo.lock"]), ORIGINAL_LOCK)
        result = self.run_runner("-p", "consensus-simtests")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_restored()
        calls = self.calls()
        self.assertEqual(calls[0]["args"], ["nextest", "--help"])
        self.assertEqual(calls[1]["args"][:4], ["nextest", "run", "--cargo-profile", "simulator"])
        self.assertEqual(calls[1]["env"]["MSIM_TEST_SEED"], "1")
        self.assertEqual(calls[1]["env"]["MSIM_WATCHDOG_TIMEOUT_MS"], "60000")
        self.assertIn("--cfg msim", calls[1]["env"]["RUSTFLAGS"])

    def test_cargo_failure_preserves_exit_code_and_restores(self):
        result = self.run_runner(FAKE_EXIT="37")
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assert_restored()

    def test_help_failure_also_restores(self):
        result = self.run_runner(FAKE_HELP_EXIT="21")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assert_restored()
        self.assertEqual(len(self.calls()), 1)

    def test_deleted_lock_is_restored(self):
        result = self.run_runner(FAKE_MODE="delete-lock")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_restored()

    def test_build_keeps_patch_flags_seed_and_timeout(self):
        local_msim = str(self.workspace / "local msim")
        result = self.run_runner(
            "build", "-p", "consensus-simtests", LOCAL_MSIM_PATH=local_msim,
            USE_MOCK_CRYPTO="1", MSIM_TEST_SEED="55", MSIM_WATCHDOG_TIMEOUT_MS="90000",
            RUSTFLAGS="-C force-frame-pointers=yes",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_restored()
        call = self.calls()[1]
        self.assertEqual(call["args"][:3], ["build", "--profile", "simulator"])
        self.assertIn('patch.crates-io.tokio.path = "' + local_msim + '/msim-tokio"', call["args"])
        self.assertIn('patch.crates-io.blst.rev = "630ca4d55de8e199e62c5b6a695c702d95fe6498"', call["args"])
        self.assertEqual(call["env"]["MSIM_TEST_SEED"], "55")
        self.assertEqual(call["env"]["MSIM_WATCHDOG_TIMEOUT_MS"], "90000")
        self.assertEqual(call["env"]["RUSTFLAGS"], "-C force-frame-pointers=yes --cfg msim --cfg use_mock_crypto")

    def test_catchable_signals_wait_for_cargo_then_restore(self):
        for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            with self.subTest(signal=signum):
                for name in ("ready", "child-stopped"):
                    (self.workspace / name).unlink(missing_ok=True)
                process = subprocess.Popen(
                    [str(RUNNER), "simtest"], cwd=self.child_dir,
                    env=dict(self.env, FAKE_MODE="signal"),
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
                )
                try:
                    deadline = time.monotonic() + 5
                    while not (self.workspace / "ready").exists():
                        if process.poll() is not None or time.monotonic() >= deadline:
                            self.fail("fake Cargo did not become ready")
                        time.sleep(0.01)
                    process.send_signal(signum)
                    _, stderr = process.communicate(timeout=5)
                    self.assertEqual(process.returncode, 128 + signum, stderr)
                    self.assertEqual((self.workspace / "child-stopped").read_text(), str(int(signum)))
                    self.assert_restored()
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.communicate()

    def test_restore_failure_keeps_backup_and_nonzero_exit(self):
        result = self.run_runner(FAKE_MODE="restore-failure")
        self.assertEqual(result.returncode, 1, result.stderr)
        backups = list((self.workspace / "tmp").glob("cargo-simtest-lock.*/Cargo.lock"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), ORIGINAL_LOCK)
        self.assertIn(str(backups[0]).encode(), result.stderr)
        self.assertEqual(list(self.lock.iterdir()), [])

    def test_restore_failure_preserves_existing_cargo_error(self):
        result = self.run_runner(FAKE_MODE="restore-failure", FAKE_EXIT="37")
        self.assertEqual(result.returncode, 37, result.stderr)
        backup = next((self.workspace / "tmp").glob("cargo-simtest-lock.*/Cargo.lock"))
        self.assertEqual(backup.read_bytes(), ORIGINAL_LOCK)

    def test_missing_lock_stops_before_cargo(self):
        self.lock.unlink()
        result = self.run_runner()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertFalse((self.workspace / "calls.jsonl").exists())


if __name__ == "__main__":
    unittest.main()
