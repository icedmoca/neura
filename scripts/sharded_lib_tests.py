#!/usr/bin/env python3
"""Run Neura library tests in deterministic shards.

Why this exists:
- `cargo test --lib` is large enough that local/agent harnesses can time out.
- Rust's default test filter is substring-based, so naive grouped filters can run
  the wrong tests or zero tests.
- Passing `--exact` lets us execute a listed test by its full libtest name.
- `CARGO_INCREMENTAL=0` avoids occasional rustc incremental verify_ich ICEs seen
  during very large test builds.

Examples:
  scripts/sharded_lib_tests.py --shard-count 8 --shard-index 0
  scripts/sharded_lib_tests.py --shard-count 8 --shard-index 0 --dry-run
  scripts/sharded_lib_tests.py --filter memory --fail-fast
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from pathlib import Path


def run(cmd: list[str], *, env: dict[str, str], timeout: int | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
    )


def extract_executable_path(cargo_output: str) -> str | None:
    for line in cargo_output.splitlines():
        line = line.strip()
        prefix = "Executable unittests src/lib.rs ("
        if line.startswith(prefix) and line.endswith(")"):
            return line[len(prefix) : -1]
    return None


def discover_tests(env: dict[str, str], manifest_path: str) -> list[str]:
    proc = run(
        ["cargo", "test", "--manifest-path", manifest_path, "--lib", "--", "--list"],
        env=env,
    )
    if proc.returncode != 0:
        print(proc.stdout, end="")
        raise SystemExit(proc.returncode)

    tests: list[str] = []
    for line in proc.stdout.splitlines():
        if line.endswith(": test"):
            tests.append(line[: -len(": test")])
    return tests


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest-path", default="Cargo.toml")
    parser.add_argument("--shard-count", type=int, default=1, help="total number of shards")
    parser.add_argument("--shard-index", type=int, default=0, help="zero-based shard index")
    parser.add_argument("--filter", default=None, help="optional substring filter before sharding")
    parser.add_argument("--fail-fast", action="store_true", help="stop after the first failed test")
    parser.add_argument("--dry-run", action="store_true", help="print selected tests without running them")
    parser.add_argument("--no-prebuild", action="store_true", help="skip cargo test --lib --no-run prebuild")
    parser.add_argument("--timeout-seconds", type=int, default=300, help="timeout per exact test")
    args = parser.parse_args()

    if args.shard_count < 1:
        parser.error("--shard-count must be >= 1")
    if args.shard_index < 0 or args.shard_index >= args.shard_count:
        parser.error("--shard-index must be in [0, shard-count)")

    repo_root = Path(__file__).resolve().parents[1]
    os.chdir(repo_root)

    env = os.environ.copy()
    env.setdefault("CARGO_INCREMENTAL", "0")

    tests = discover_tests(env, args.manifest_path)
    if args.filter:
        tests = [test for test in tests if args.filter in test]

    selected = [test for idx, test in enumerate(tests) if idx % args.shard_count == args.shard_index]

    print(
        f"Discovered {len(tests)} matching library tests; "
        f"running shard {args.shard_index + 1}/{args.shard_count} with {len(selected)} tests",
        flush=True,
    )

    if args.dry_run:
        for test in selected:
            print(test)
        return 0

    if not selected:
        return 0

    test_executable: str | None = None
    if not args.no_prebuild:
        print("Prebuilding library test binary with CARGO_INCREMENTAL=0...", flush=True)
        prebuild = run(
            ["cargo", "test", "--manifest-path", args.manifest_path, "--lib", "--no-run"],
            env=env,
        )
        print(prebuild.stdout, end="")
        if prebuild.returncode != 0:
            return prebuild.returncode
        test_executable = extract_executable_path(prebuild.stdout)
        if test_executable is None:
            print("Could not find libtest executable path in cargo output", file=sys.stderr)
            return 1

    failures: list[tuple[str, int | str]] = []
    start = time.perf_counter()
    for ordinal, test in enumerate(selected, 1):
        print(f"[{ordinal}/{len(selected)}] {test}", flush=True)
        if test_executable is None:
            cmd = [
                "cargo",
                "test",
                "--manifest-path",
                args.manifest_path,
                "--lib",
                test,
                "--",
                "--exact",
            ]
        else:
            cmd = [test_executable, test, "--exact"]
        try:
            proc = run(cmd, env=env, timeout=args.timeout_seconds)
        except subprocess.TimeoutExpired as exc:
            print(exc.stdout or "", end="")
            print(f"TIMEOUT after {args.timeout_seconds}s: {test}", file=sys.stderr, flush=True)
            failures.append((test, "timeout"))
            if args.fail_fast:
                break
            continue

        if proc.returncode != 0:
            print(proc.stdout, end="")
            failures.append((test, proc.returncode))
            if args.fail_fast:
                break

    elapsed = time.perf_counter() - start
    if failures:
        print(f"\nFAILED shard {args.shard_index + 1}/{args.shard_count} in {elapsed:.1f}s")
        for test, reason in failures:
            print(f"- {test}: {reason}")
        return 1

    print(f"\nOK shard {args.shard_index + 1}/{args.shard_count} in {elapsed:.1f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
