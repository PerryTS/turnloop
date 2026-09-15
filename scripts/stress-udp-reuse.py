#!/usr/bin/env python3
"""Run the Unix UDP regressions while threads churn exclusive ephemeral ports."""

import argparse
from collections import deque
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import re
import socket
import subprocess
import threading


def churn(family, host, stop, ready):
    held = deque()
    count = 0
    try:
        while not stop.is_set():
            sock = socket.socket(family, socket.SOCK_DGRAM)
            try:
                sock.bind((host, 0))
            except BaseException:
                sock.close()
                raise
            held.append(sock)
            count += 1
            if len(held) > 128:
                held.popleft().close()
            ready.set()
    finally:
        for sock in held:
            sock.close()
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iterations", type=int, default=2000)
    parser.add_argument("--binary", type=Path, help="Use a previously built test binary")
    parser.add_argument("--logs", type=Path, default=Path(".tools/udp-fix/stress"))
    parser.add_argument("--require-contention", action="store_true",
                        help="Fail unless a real released-port bind failed during a passing suite")
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("--iterations must be positive")
    args.logs.mkdir(parents=True, exist_ok=True)
    binary = args.binary
    if binary is None:
        command = ["cargo", "test", "--locked", "-p", "turnloop", "--lib",
                   "--no-run", "--message-format=json"]
        print("Building:", " ".join(command), flush=True)
        build = subprocess.run(command, text=True, stdout=subprocess.PIPE, check=True)
        artifacts = [json.loads(line) for line in build.stdout.splitlines()]
        binaries = [a["executable"] for a in artifacts
                    if a.get("reason") == "compiler-artifact"
                    and a["target"]["name"] == "turnloop" and a.get("executable")]
        if len(binaries) != 1:
            raise RuntimeError(f"expected one core test executable: {binaries}")
        binary = Path(binaries[0])
    command = [str(binary.resolve()), "backend::unix::udp_tests::", "--nocapture"]
    stop = threading.Event()
    workers = [(socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")] * 2
    status = 0
    counts = []
    recovered = 0
    with ThreadPoolExecutor(max_workers=len(workers)) as pool:
        ready = [threading.Event() for _ in workers]
        futures = [pool.submit(churn, family, host, stop, signal)
                   for (family, host), signal in zip(workers, ready)]
        try:
            for signal in ready:
                if not signal.wait(10):
                    raise RuntimeError("ephemeral-port worker did not bind")
            for iteration in range(1, args.iterations + 1):
                for future in futures:
                    if future.done():
                        future.result()
                        raise RuntimeError("port churn stopped before the tests")
                run = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, timeout=30)
                (args.logs / f"{iteration:05}.log").write_text(run.stdout)
                passed = re.search(r"test result: ok\. ([1-9]\d*) passed; 0 failed; 0 ignored;",
                                   run.stdout)
                subjects = ["default_udp_bind_does_not_enable_address_sharing",
                            "udp_port_sharing_requires_explicit_opt_in",
                            "cancelled_udp_with_cached_events_survives_exact_fd_and_port_reuse"]
                ran = all(f"test backend::unix::udp_tests::{name} ... ok" in run.stdout
                          for name in subjects)
                if run.returncode or not passed or not ran:
                    print(run.stdout, flush=True)
                    print(f"FAIL iteration {iteration}: exit={run.returncode}", flush=True)
                    status = 1
                    break
                recovered += len(re.findall(r"(?:bind reused fd|rebind released endpoint) .*Error \{",
                                            run.stdout))
                if iteration % 100 == 0 or iteration == args.iterations:
                    print(f"PASS {iteration}/{args.iterations} UDP suites", flush=True)
        finally:
            stop.set()
            counts = [future.result() for future in futures]
    if not all(count > 0 for count in counts):
        raise RuntimeError(f"churn did not execute on every thread: {counts}")
    print(f"Exclusive ephemeral binds per worker (IPv4, IPv6, IPv4, IPv6): {counts}", flush=True)
    print(f"Real rebind failures recovered in passing suites: {recovered}", flush=True)
    if args.require_contention and not recovered:
        print("FAIL: no actual released-port contention observed", flush=True)
        status = 1
    return status


if __name__ == "__main__":
    raise SystemExit(main())
