#!/usr/bin/env python3
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0

"""Own a four-validator local cluster and control only its direct child processes."""

import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import sys
import time
import urllib.request


SCRIPT = Path(__file__).resolve()
REPO = SCRIPT.parents[2]


def configurations(directory):
    return [json.loads((directory / "network" / f"node{i}" / "config.json").read_text()) for i in range(4)]


def endpoint(config):
    return "http://" + config["api_address"]


def http_status(config):
    with urllib.request.urlopen(endpoint(config) + "/status", timeout=3) as response:
        return json.load(response)


def control(directory, action, **fields):
    token = (directory / ".control-token").read_text()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(30)
        connection.connect(str(directory / "control.sock"))
        connection.sendall(json.dumps({"token": token, "action": action, **fields}).encode() + b"\n")
        reader = connection.makefile("rb")
        response = json.loads(reader.readline(1024 * 1024))
    if not response.get("ok"):
        raise RuntimeError(response.get("error", "cluster control failed"))
    return response


def reserve_ports(ports):
    reservations = {}
    try:
        for port in ports:
            listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                listener.bind(("127.0.0.1", port))
                listener.listen(1)
            except BaseException:
                listener.close()
                raise
            reservations[port] = listener
        return reservations
    except BaseException:
        for listener in reservations.values():
            listener.close()
        raise


def allocate_ports(requested):
    for _ in range(100):
        base = requested if requested is not None else 20000 + secrets.randbelow(30000)
        ports = [base + offset + index for offset in (0, 10, 20) for index in range(4)]
        try:
            return base, reserve_ports(ports)
        except OSError:
            if requested is not None:
                raise
    raise RuntimeError("could not reserve twelve local cluster ports")


def stop_child(child):
    # Popen owns an unreaped direct child; no PID read from disk is ever signalled.
    if child.poll() is None:
        child.send_signal(signal.SIGINT)
        try:
            child.wait(timeout=8)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)


def supervise(args):
    directory = args.dir.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    lock = (directory / ".cluster.lock").open("a")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    binary_directory = args.bin_dir.resolve()
    node = binary_directory / "dagora-node"
    application = binary_directory / "dagora-abci-mock"
    if not node.is_file() or not application.is_file():
        raise RuntimeError(f"build dagora-node and dagora-abci-mock first; binaries expected in {binary_directory}")
    reservations = {}
    children = {}
    logs = []
    server = None
    socket_path = directory / "control.sock"
    stopping = False

    def interrupt(_signal, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)

    def release(port):
        listener = reservations.pop(port, None)
        if listener is not None:
            listener.close()

    def spawn(role, index, fail_once=False):
        key = f"{role}{index}"
        current = children.get(key)
        if current is not None and current.poll() is None:
            raise RuntimeError(f"{key} is already running")
        config = configs[index]
        if role == "app":
            port = int(config["abci_address"].rsplit(":", 1)[1])
            command = [str(application), "--listen", config["abci_address"], "--data-dir", str(directory / "network" / f"node{index}" / "app")]
            if fail_once:
                command.append("--fail-after-commit-once")
            ports = [port]
        else:
            ports = [config["consensus_port"], int(config["api_address"].rsplit(":", 1)[1])]
            command = [str(node), "run", "--config", str(directory / "network" / f"node{index}" / "config.json")]
        missing = [port for port in ports if port not in reservations]
        reservations.update(reserve_ports(missing))
        for port in ports:
            release(port)
        log = (directory / "logs" / f"{key}.log").open("ab", buffering=0)
        logs.append(log)
        children[key] = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log)

    def state():
        return {key: {"pid": child.pid, "running": child.poll() is None, "exit_code": child.poll()} for key, child in children.items()}

    try:
        (directory / "logs").mkdir(exist_ok=True)
        if not (directory / "network" / "committee.json").exists():
            base, reservations = allocate_ports(args.port_base)
            subprocess.run([str(node), "init", "--dir", str(directory / "network"), "--port-base", str(base)], check=True, stdout=subprocess.DEVNULL)
        configs = configurations(directory)
        if not reservations:
            ports = []
            for config in configs:
                ports += [config["consensus_port"], int(config["api_address"].rsplit(":", 1)[1]), int(config["abci_address"].rsplit(":", 1)[1])]
            reservations = reserve_ports(ports)
        with contextlib.suppress(FileNotFoundError):
            socket_path.unlink()
        token = secrets.token_hex(32)
        descriptor = os.open(directory / ".control-token", os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
        with os.fdopen(descriptor, "w") as handle:
            handle.write(token)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(socket_path))
        socket_path.chmod(0o600)
        server.listen(4)
        server.settimeout(0.5)
        for index in range(4):
            spawn("app", index)
        for index in range(4):
            spawn("node", index)
        while not stopping:
            try:
                connection, _ = server.accept()
            except socket.timeout:
                continue
            with connection:
                connection.settimeout(5)
                try:
                    request = json.loads(connection.makefile("rb").readline(4096))
                    if not secrets.compare_digest(str(request.get("token", "")), token):
                        raise RuntimeError("invalid cluster control token")
                    action = request.get("action")
                    if action == "stop":
                        stopping = True
                    elif action != "status":
                        operation, role = action.split("-", 1)
                        index = request["index"]
                        if role not in ("node", "app") or operation not in ("stop", "restart") or type(index) is not int or not 0 <= index < 4:
                            raise RuntimeError("invalid cluster child operation")
                        if operation == "stop":
                            stop_child(children[f"{role}{index}"])
                        else:
                            spawn(role, index, request.get("fail_after_commit_once", False))
                    connection.sendall(json.dumps({"ok": True, "children": state()}).encode() + b"\n")
                except Exception as error:
                    connection.sendall(json.dumps({"ok": False, "error": str(error)}).encode() + b"\n")
    finally:
        for role in ("node", "app"):
            for index in range(4):
                child = children.get(f"{role}{index}")
                if child is not None:
                    stop_child(child)
        for listener in reservations.values():
            listener.close()
        for log in logs:
            log.close()
        if server is not None:
            server.close()
            with contextlib.suppress(FileNotFoundError):
                socket_path.unlink()
        lock.close()


def start(args):
    directory = args.dir.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    if (directory / "control.sock").exists():
        try:
            control(directory, "status")
        except (OSError, RuntimeError):
            pass
        else:
            raise RuntimeError("a cluster already owns this directory")
    with (directory / "supervisor.log").open("ab", buffering=0) as log:
        command = [sys.executable, str(SCRIPT), "supervise", "--dir", str(directory), "--bin-dir", str(args.bin_dir.resolve())]
        if args.port_base is not None:
            command += ["--port-base", str(args.port_base)]
        supervisor = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
    deadline = time.monotonic() + 90
    last_error = None
    while time.monotonic() < deadline:
        if supervisor.poll() is not None:
            raise RuntimeError(f"cluster supervisor exited; inspect {directory / 'supervisor.log'}")
        try:
            children = control(directory, "status")["children"]
            if len(children) != 8 or not all(child["running"] for child in children.values()):
                raise RuntimeError(f"cluster child exited; inspect logs in {directory / 'logs'}")
            states = [http_status(config) for config in configurations(directory)]
            if all(state["execution_height"] > 0 for state in states):
                return {"ok": True, "directory": str(directory), "urls": [endpoint(config) for config in configurations(directory)], "heights": [state["execution_height"] for state in states]}
        except Exception as error:
            last_error = error
        time.sleep(0.25)
    with contextlib.suppress(Exception):
        control(directory, "stop")
    raise RuntimeError(f"cluster startup timed out: {last_error}; inspect {directory / 'logs'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["start", "stop", "status", "stop-node", "restart-node", "stop-app", "restart-app", "supervise"])
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--bin-dir", type=Path, default=REPO / "target" / "debug")
    parser.add_argument("--port-base", type=int)
    parser.add_argument("--index", type=int)
    parser.add_argument("--fail-after-commit-once", action="store_true")
    args = parser.parse_args()
    if args.action == "supervise":
        supervise(args)
    elif args.action == "start":
        print(json.dumps(start(args), indent=2))
    else:
        fields = {}
        if args.action not in ("stop", "status"):
            if args.index is None:
                parser.error("child operations require --index 0..3")
            fields = {"index": args.index, "fail_after_commit_once": args.fail_after_commit_once}
        directory = args.dir.resolve()
        response = control(directory, args.action, **fields)
        if args.action == "stop":
            deadline = time.monotonic() + 45
            while (directory / "control.sock").exists():
                if time.monotonic() >= deadline:
                    raise RuntimeError("cluster shutdown did not finish within 45 seconds")
                time.sleep(0.1)
            response["stopped"] = True
        print(json.dumps(response, indent=2))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
