#!/usr/bin/env python3
"""Build and freeze release benchmarks with source, binary, and host provenance."""

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import shutil
import subprocess
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parent.parent
BENCHMARK = "graphql-static-analysis-benchmark"


def timestamp():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def command_output(*command, cwd=None):
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def new_directory(path):
    path = path.resolve()
    if path.is_relative_to(REPOSITORY) and not path.is_relative_to(REPOSITORY / ".scratch"):
        raise ValueError("store measurement artifacts under .scratch/ or outside the repository")
    path.mkdir(parents=True, exist_ok=False)
    return path


def host_details():
    host = {
        "hostname": platform.node(),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "processor": platform.processor(),
    }
    return host


def hardware_details():
    # Optional metadata may be unavailable in a sandbox. It is separate from the
    # stable host identity so sampling permissions do not change comparability.
    details = {}
    if platform.system() == "Darwin":
        for key, name in [("processor", "machdep.cpu.brand_string"), ("memory_bytes", "hw.memsize")]:
            result = subprocess.run(["sysctl", "-n", name], text=True, capture_output=True)
            details[key] = result.stdout.strip() if result.returncode == 0 else None
    return details


def source_hashes(repository):
    # The standalone package resolves its own lockfile. Include all Rust source and
    # build configuration; generated targets and measurement outputs are excluded.
    paths = [repository / "Cargo.toml", repository / "benchmarks/Cargo.toml",
             repository / "benchmarks/Cargo.lock"]
    for folder in ["src", "benchmarks/src"]:
        paths.extend(p for p in (repository / folder).rglob("*") if p.is_file())
    for name in ["build.rs", "benchmarks/build.rs", "rust-toolchain", "rust-toolchain.toml",
                 "README.md", ".cargo/config", ".cargo/config.toml",
                 "benchmarks/.cargo/config", "benchmarks/.cargo/config.toml"]:
        path = repository / name
        if path.is_file():
            paths.append(path)
    return {str(p.relative_to(repository)): sha256(p) for p in sorted(set(paths))}


def capture(output, offline=False):
    output = new_directory(output)
    source = source_hashes(REPOSITORY)
    build_command = ["cargo", "build", "--release", "--locked",
                     "--message-format=json-render-diagnostics"]
    if offline:
        build_command.append("--offline")
    metadata = {
        "format": 1,
        "created_at_utc": timestamp(),
        "git_revision": command_output("git", "rev-parse", "HEAD", cwd=REPOSITORY),
        "git_status": command_output("git", "status", "--short", cwd=REPOSITORY),
        "source_sha256": source,
        "host": host_details(),
        "hardware": hardware_details(),
        "rustc": command_output("rustc", "-Vv", cwd=REPOSITORY / "benchmarks"),
        "cargo": command_output("cargo", "-V", cwd=REPOSITORY / "benchmarks"),
        "build_command": build_command,
        "build_environment": {k: v for k, v in os.environ.items() if k in {
            "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN", "RUSTC",
            "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
        } or k.startswith("CARGO_PROFILE_RELEASE_")},
    }
    # Compilation completes before any timing/profiling command can use this artifact.
    build = subprocess.run(build_command, cwd=REPOSITORY / "benchmarks", text=True,
                           stdout=subprocess.PIPE, check=True)
    (output / "build.jsonl").write_text(build.stdout)
    binaries = [event["executable"] for line in build.stdout.splitlines()
                if (event := json.loads(line)).get("reason") == "compiler-artifact"
                and event.get("target", {}).get("name") == BENCHMARK
                and event.get("executable")]
    if len(binaries) != 1:
        raise RuntimeError(f"expected one {BENCHMARK} artifact, got {binaries}")
    if source_hashes(REPOSITORY) != source:
        raise RuntimeError("source changed during the build; capture a fresh artifact")
    shutil.copy2(binaries[0], output / "benchmark")
    for name in source:
        destination = output / "source" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPOSITORY / name, destination)
    metadata["binary_sha256"] = sha256(output / "benchmark")
    write_json(output / "manifest.json", metadata)
    verify_snapshot(output)
    return output


def verify_snapshot(path):
    path = path.resolve()
    metadata = json.loads((path / "manifest.json").read_text())
    if metadata.get("format") != 1:
        raise ValueError(f"unsupported artifact format: {path}")
    if sha256(path / "benchmark") != metadata["binary_sha256"]:
        raise ValueError(f"binary hash mismatch: {path}")
    for name, expected in metadata["source_sha256"].items():
        source = (path / "source" / name).resolve()
        if not source.is_relative_to(path / "source") or sha256(source) != expected:
            raise ValueError(f"source hash mismatch: {name}")
    return metadata


def require_comparable(before, after):
    for key in ["rustc", "cargo", "build_environment", "host"]:
        if before[key] != after[key]:
            raise ValueError(f"snapshots have different {key}")
    def harness(metadata):
        return {k: v for k, v in metadata["source_sha256"].items()
                if k.startswith("benchmarks/")}
    if harness(before) != harness(after):
        raise ValueError("benchmark source or dependency lockfile differs")
    if before["host"] != host_details():
        raise ValueError("measurement host differs from the snapshot host")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--offline", action="store_true")
    arguments = parser.parse_args()
    print(capture(arguments.output_dir, arguments.offline))


if __name__ == "__main__":
    main()
