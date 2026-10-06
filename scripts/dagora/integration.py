#!/usr/bin/env python3
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0

"""Exercise real local Mysticeti consensus and ABCI RPC, including a 600s soak by default."""

import argparse
import contextlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.error
import urllib.request
import uuid

from cluster import REPO, configurations, control, endpoint


def stage(message):
    print(f"[integration] {message}", flush=True)


def fetch(url, transaction=None):
    body = None if transaction is None else json.dumps({"tx": transaction}).encode()
    request = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)


def wait_for(description, predicate, timeout=60):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError, AssertionError) as error:
            last_error = error
        time.sleep(0.25)
    raise AssertionError(f"timed out waiting for {description}: {last_error}")


def states(urls, indices):
    return [fetch(urls[index] + "/status") for index in indices]


def check_prefix(urls, indices):
    snapshots = states(urls, indices)
    height = min(state["execution_height"] for state in snapshots)
    assert height > 0, "cluster did not produce an empty block"
    reference = fetch(urls[indices[0]] + f"/blocks/{height}")
    for index in indices[1:]:
        candidate = fetch(urls[index] + f"/blocks/{height}")
        assert candidate == reference, f"block/receipt mismatch at height {height}, validator {index}"
    for state in snapshots:
        assert state["epoch"] == 0 and state["chain_id"] == "dagora-local"
        assert state["consensus_commit_index"] >= state["execution_commit_index"]
    return height


def wait_transaction(url, transaction, start_height, expected_code):
    scanned = start_height

    def included():
        nonlocal scanned
        height = fetch(url + "/status")["execution_height"]
        for block_height in range(scanned + 1, height + 1):
            record = fetch(url + f"/blocks/{block_height}")
            for index, tx in enumerate(record["block"]["transactions"]):
                if bytes(tx) == transaction.encode():
                    assert record["execution"]["tx_results"][index]["code"] == expected_code
                    return block_height
            scanned = block_height
        return None

    return wait_for(f"transaction execution with code {expected_code}", included)


def run(args):
    directory = (args.dir or Path(tempfile.mkdtemp(prefix="dagora-integration-"))).resolve()
    cluster_script = Path(__file__).with_name("cluster.py")
    command = [sys.executable, str(cluster_script), "start", "--dir", str(directory), "--bin-dir", str(args.bin_dir.resolve())]
    stage(f"Starting cluster in {directory}")
    subprocess.run(command, check=True)
    urls = [endpoint(config) for config in configurations(directory)]
    transitions = {}
    try:
        stage("Checking ownership, block agreement, and transaction receipts")
        # A second starter must not disturb processes already owned by the supervisor.
        duplicate = subprocess.run(command, capture_output=True, text=True)
        assert duplicate.returncode != 0, "duplicate cluster start unexpectedly succeeded"
        initial = wait_for("initial shared block prefix", lambda: check_prefix(urls, [0, 1, 2, 3]))
        transitions["initial"] = initial
        token = uuid.uuid4().hex
        before = fetch(urls[0] + "/status")["execution_height"]
        good = "ok:" + token
        failed = "fail:" + token
        fetch(urls[0] + "/transactions", good)
        good_height = wait_transaction(urls[0], good, before, 0)
        before = fetch(urls[0] + "/status")["execution_height"]
        fetch(urls[0] + "/transactions", failed)
        failed_height = wait_transaction(urls[0], failed, before, 1)
        wait_for("all validators executing success/failure fixtures", lambda: min(state["execution_height"] for state in states(urls, range(4))) >= failed_height)
        for height in (good_height, failed_height):
            records = [fetch(url + f"/blocks/{height}") for url in urls]
            assert records.count(records[0]) == 4, "execution receipts diverged"
        try:
            fetch(urls[0] + "/transactions", "")
        except urllib.error.HTTPError as error:
            assert error.code == 400
        else:
            raise AssertionError("empty ingress transaction was accepted")
        transitions["transactions"] = failed_height

        stage("Checking progress with one validator offline")
        control(directory, "stop-node", index=3)
        baseline = min(state["execution_height"] for state in states(urls, [0, 1, 2]))
        wait_for("progress with one validator offline", lambda: min(state["execution_height"] for state in states(urls, [0, 1, 2])) >= baseline + 3)
        transitions["one_offline"] = check_prefix(urls, [0, 1, 2])

        stage("Checking consensus and execution stop with two validators offline")
        control(directory, "stop-node", index=2)
        # Already committed outputs can drain after quorum is lost. Require a stable 5s window.
        freeze_deadline = time.monotonic() + 60
        while True:
            snapshot = [(state["consensus_commit_index"], state["execution_height"]) for state in states(urls, [0, 1])]
            time.sleep(5)
            after = [(state["consensus_commit_index"], state["execution_height"]) for state in states(urls, [0, 1])]
            if after == snapshot:
                break
            assert time.monotonic() < freeze_deadline, "two-validator minority kept committing"
        frozen_height = min(height for _, height in after)
        transitions["two_offline_frozen"] = frozen_height
        stage(f"Both cursors stopped; restarting validators at execution height {frozen_height}")
        control(directory, "restart-node", index=2)
        control(directory, "restart-node", index=3)
        wait_for("restarted validators catching up", lambda: min(state["execution_height"] for state in states(urls, range(4))) >= max(height for _, height in after) + 3, timeout=90)
        transitions["node_restart"] = check_prefix(urls, [0, 1, 2, 3])

        stage("Checking application loss, validator fail-stop, and restart recovery")
        control(directory, "stop-app", index=0)
        wait_for("node fail-stop after ABCI loss", lambda: not control(directory, "status")["children"]["node0"]["running"], timeout=25)
        baseline = min(state["execution_height"] for state in states(urls, [1, 2, 3]))
        wait_for("other validators continuing after application loss", lambda: min(state["execution_height"] for state in states(urls, [1, 2, 3])) >= baseline + 3)
        control(directory, "restart-app", index=0)
        control(directory, "restart-node", index=0)
        wait_for("ABCI and node restart recovery", lambda: min(state["execution_height"] for state in states(urls, range(4))) >= baseline + 3, timeout=90)
        transitions["application_restart"] = check_prefix(urls, [0, 1, 2, 3])

        stage("Checking recovery after application Commit persistence without an RPC reply")
        # Kill the mock after application persistence but before its Commit RPC reply.
        control(directory, "stop-node", index=0)
        control(directory, "stop-app", index=0)
        control(directory, "restart-app", index=0, fail_after_commit_once=True)
        control(directory, "restart-node", index=0)
        wait_for("uncertain Commit response failure", lambda: not control(directory, "status")["children"]["app0"]["running"], timeout=30)
        wait_for("node stopping after uncertain Commit", lambda: not control(directory, "status")["children"]["node0"]["running"], timeout=25)
        control(directory, "restart-app", index=0)
        catchup_target = max(state["execution_height"] for state in states(urls, [1, 2, 3]))
        control(directory, "restart-node", index=0)
        wait_for("Info-based uncertain Commit recovery", lambda: min(state["execution_height"] for state in states(urls, range(4))) >= catchup_target, timeout=90)
        transitions["uncertain_commit_restart"] = check_prefix(urls, [0, 1, 2, 3])

        stage(f"Starting {args.duration:g}-second agreement and progress soak")
        start = time.monotonic()
        last_submission = start - 5
        next_report = start + 60
        baseline = check_prefix(urls, [0, 1, 2, 3])
        while time.monotonic() - start < args.duration:
            if time.monotonic() - last_submission >= 5:
                fetch(urls[0] + "/transactions", "soak:" + uuid.uuid4().hex)
                last_submission = time.monotonic()
            height = check_prefix(urls, [0, 1, 2, 3])
            assert height >= baseline, "execution height went backwards"
            if time.monotonic() >= next_report:
                stage(f"Soak running for {time.monotonic() - start:.0f}s; common execution height {height}")
                next_report += 60
            time.sleep(1)
        final = check_prefix(urls, [0, 1, 2, 3])
        if args.duration >= 5:
            assert final > baseline, "soak cluster did not advance"
        transitions["soak_final"] = final

        stage("Checking whole-cluster restart with the same persisted data")
        persisted = fetch(urls[0] + f"/blocks/{final}")
        previous_max = max(state["execution_height"] for state in states(urls, range(4)))
        subprocess.run([sys.executable, str(cluster_script), "stop", "--dir", str(directory)], check=True, timeout=50)
        subprocess.run(command, check=True)
        wait_for("whole-cluster persisted recovery and progress", lambda: min(state["execution_height"] for state in states(urls, range(4))) > previous_max, timeout=90)
        for url in urls:
            assert fetch(url + f"/blocks/{final}") == persisted, "persisted block changed after whole-cluster restart"
        transitions["whole_cluster_restart"] = check_prefix(urls, [0, 1, 2, 3])
        print(json.dumps({"ok": True, "directory": str(directory), "soak_seconds": args.duration, "transitions": transitions}, indent=2))
    finally:
        stage("Stopping owned cluster processes")
        with contextlib.suppress(Exception):
            subprocess.run([sys.executable, str(cluster_script), "stop", "--dir", str(directory)], check=True, timeout=50)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path)
    parser.add_argument("--bin-dir", type=Path, default=REPO / "target" / "debug")
    parser.add_argument("--duration", type=float, default=600, help="soak seconds after failure/restart checks (default: 600)")
    args = parser.parse_args()
    if args.duration < 0:
        parser.error("duration must be nonnegative")
    run(args)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"integration failed: {error}", file=sys.stderr)
        traceback.print_exc()
        sys.exit(1)
