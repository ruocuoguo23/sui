#!/usr/bin/env python3
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0

"""Check determinism-tool outcomes using a fake, already-built test executable."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


CHECKER = Path(__file__).with_name("check-determinism.sh").resolve()
FAKE_TEST = r'''#!/usr/bin/env python3
import os
from pathlib import Path
import sys

expected = ["--test-threads", "1", "--nocapture", "--exact", "consensus_tests::test_committee_start_simple"]
assert sys.argv[1:] == expected
assert os.environ["MSIM_TEST_SEED"] == "44"
assert "consensus_core=debug" in os.environ["RUST_LOG"]
mode = os.environ.get("FAKE_TEST_MODE", "success")
counter = Path(os.environ["FAKE_COUNTER"])
count = int(counter.read_text()) if counter.exists() else 0
counter.write_text(str(count + 1))
print("running 0 tests" if mode == "missing-test" else "running 1 test")
print("ThreadId(" + str(os.getpid()) + ") consensus block committed")
if mode == "diverge":
    print("commit digest differs: " + str(count))
print("test result: ok. finished in 1.23s")
sys.exit(42 if mode == "fail" else 0)
'''


class CheckDeterminismTests(unittest.TestCase):
    def setUp(self):
        git = shutil.which("git")
        if git is None:
            self.skipTest("git is required to locate the temporary repository")
        self.temporary = tempfile.TemporaryDirectory(prefix="simtest-determinism-test-")
        self.addCleanup(self.temporary.cleanup)
        self.workspace = Path(self.temporary.name)
        subprocess.run([git, "init", "--quiet", str(self.workspace)], check=True)
        self.target = self.workspace / "custom-target"
        binaries = self.target / "simulator" / "deps"
        binaries.mkdir(parents=True)
        self.binary = binaries / "consensus_tests-0123456789abcdef"
        self.binary.write_text(FAKE_TEST)
        self.binary.chmod(0o755)
        self.env = dict(
            os.environ, CARGO_TARGET_DIR=str(self.target), MSIM_TEST_SEED="44",
            FAKE_COUNTER=str(self.workspace / "counter"),
        )
        self.env.pop("BINARY_NAME", None)

    def run_checker(self, mode="success"):
        return subprocess.run(
            [str(CHECKER)], cwd=self.workspace,
            env=dict(self.env, FAKE_TEST_MODE=mode),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=10,
        )

    def test_equal_successful_runs_pass_with_custom_target_dir(self):
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("PASS: identical output", result.stdout)
        self.assertEqual((self.workspace / "counter").read_text(), "2")

    def test_differing_successful_runs_fail(self):
        result = self.run_checker("diverge")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("output diverged", result.stdout)

    def test_equal_failed_runs_still_fail(self):
        result = self.run_checker("fail")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("exit codes: 42, 42", result.stdout)

    def test_missing_test_is_rejected(self):
        result = self.run_checker("missing-test")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("expected 1 determinism tests, but ran 0", result.stdout)

    def test_missing_binary_is_rejected(self):
        self.binary.unlink()
        result = self.run_checker()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("could not find an executable 'consensus_tests'", result.stdout)


if __name__ == "__main__":
    unittest.main()
