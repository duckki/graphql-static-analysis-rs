#!/usr/bin/env python3
"""Sample reusable estimate calls in a frozen release artifact (macOS sample)."""

import argparse
import csv
import platform
import re
import subprocess
from collections import defaultdict
from pathlib import Path

from artifacts import host_details, new_directory, timestamp, verify_snapshot, write_json

WORKLOADS = {
    "schema-supplied": ["profile", "10240", "8", "exact-case", "with-values", "1000000"],
    "query-supplied": ["profile", "1024", "80", "exact-case", "with-values", "1000000"],
    "booleans-supplied": ["profile-pathological-booleans", "6", "exact-case", "with-values", "1000000"],
    "schema-symbolic": ["profile", "10240", "8", "exact-case", "without-values", "1000000"],
    "booleans-symbolic": ["profile-pathological-booleans", "6", "exact-case", "without-values", "1000000"],
}
# Inclusive stack membership: categories overlap; recursion counts a sample once.
CATEGORIES = {
    "extraction": ("condition_tree::",),
    "intersection": ("PossibleTypesMap::intersection",),
    "type_partition": ("split_possible_type_region", "partition_type_region"),
    "schema_lookup": ("Schema::type_field",),
    "fingerprint": ("possible_type_fingerprint",),
    "boolean_decisions": ("BooleanDecision",),
    "allocator": ("libsystem_malloc.dylib",),
    "name_clone_drop": ("Name$u20$as$u20$core..clone..Clone", "Name$u20$as$u20$core..ops..drop..Drop"),
}


def parse_sample(text):
    graph = text.split("Call graph:\n", 1)[1].split("Total number in stack", 1)[0]
    stack, nodes = [], []
    for line in graph.splitlines():
        match = re.match(r"^([ +!:|]*)(\d+) (.*)$", line)
        if not match:
            continue
        depth = len(match[1])
        while stack and stack[-1]["depth"] >= depth:
            stack.pop()
        node = dict(depth=depth, count=int(match[2]), name=match[3], children=[],
                    parent=stack[-1] if stack else None)
        if stack:
            stack[-1]["children"].append(node)
        nodes.append(node)
        stack.append(node)
    totals = {key: 0 for key in ["estimate", *CATEGORIES]}
    top = defaultdict(int)
    for node in nodes:
        weight = node["count"] - sum(child["count"] for child in node["children"])
        if weight < 0:
            raise ValueError("invalid sample hierarchy: child counts exceed parent")
        names, ancestor = [], node
        while ancestor:
            names.append(ancestor["name"])
            ancestor = ancestor["parent"]
        if not any("MaxResponseSizeEstimator::estimate" in name for name in names):
            continue
        totals["estimate"] += weight
        for label, symbols in CATEGORIES.items():
            if any(symbol in name for name in names for symbol in symbols):
                totals[label] += weight
        name = re.sub(r"::h[0-9a-f]{16}\b", "", node["name"].split("  (in ")[0])
        top[name] += weight
    if not totals["estimate"]:
        raise ValueError("capture contains no identifiable estimate stacks")
    percentages = {key + "_percent": 100 * totals[key] / totals["estimate"] for key in CATEGORIES}
    return {**totals, **percentages}, sorted(top.items(), key=lambda item: -item[1])[:25]


def profile(snapshot, output, workloads, replicates, seconds, interval_ms):
    snapshot = snapshot.resolve()
    metadata = verify_snapshot(snapshot)
    if metadata["host"] != host_details():
        raise ValueError("measurement host differs from the snapshot host")
    output = new_directory(output)
    manifest = dict(started_at_utc=timestamp(), snapshot=metadata, seconds=seconds,
                    interval_ms=interval_ms, status="running", runs=[])
    rows = []
    try:
        for name in workloads:
            for replicate in range(1, replicates + 1):
                if verify_snapshot(snapshot) != metadata:
                    raise ValueError("snapshot changed during profiling")
                label = f"{name}-{replicate}"
                sample = output / f"{label}.sample.txt"
                write_json(output / "manifest.json", manifest)
                with (output / f"{label}.log").open("w") as log:
                    process = subprocess.Popen([str(snapshot / "benchmark"), *WORKLOADS[name]],
                                               stdout=log, stderr=subprocess.STDOUT)
                    try:
                        subprocess.run(["sample", str(process.pid), str(seconds), str(interval_ms),
                                        "-mayDie", "-file", str(sample)],
                                       stdout=log, stderr=subprocess.STDOUT, check=True)
                    finally:
                        # Stop only the child benchmark launched for this capture.
                        if process.poll() is None:
                            process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait()
                totals, top = parse_sample(sample.read_text())
                rows.append({"profile": label, **totals})
                write_json(output / f"{label}.top.json", top)
                manifest["runs"].append(dict(workload=name, replicate=replicate,
                                              arguments=WORKLOADS[name], sample=sample.name,
                                              finished_at_utc=timestamp()))
                print(f"Sampled {label}: {totals['estimate']} estimate samples", flush=True)
        with (output / "cpu-summary.csv").open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
        manifest["status"] = "complete"
    except BaseException:
        manifest["status"] = "failed"
        raise
    finally:
        manifest["finished_at_utc"] = timestamp()
        write_json(output / "manifest.json", manifest)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--workloads", nargs="+", choices=WORKLOADS, default=list(WORKLOADS))
    parser.add_argument("--replicates", type=int, default=2)
    parser.add_argument("--seconds", type=int, default=5)
    parser.add_argument("--interval-ms", type=int, default=1)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("native sampling requires macOS and its sample command")
    if args.replicates < 1 or not 1 <= args.seconds <= 60 or args.interval_ms < 1:
        parser.error("use positive replicates/interval and a duration of 1–60 seconds")
    if len(set(args.workloads)) != len(args.workloads):
        parser.error("workloads must be unique")
    profile(args.snapshot, args.output_dir, args.workloads, args.replicates,
            args.seconds, args.interval_ms)


if __name__ == "__main__":
    main()
