#!/usr/bin/env python3
"""Compare frozen release artifacts in alternating fresh processes."""

import argparse
import csv
import math
import statistics
import subprocess
from collections import defaultdict
from pathlib import Path

from artifacts import (host_details, new_directory, require_comparable, sha256,
                       timestamp, verify_snapshot, write_json)

AXES = {"schema-size": 40, "query-size": 40, "endpoints": 12,
        "pathological-booleans": 24, "cost-schema-size": 20,
        "cost-query-size": 20, "cost-endpoints": 6}
DEFAULT_AXES = ["schema-size", "query-size", "pathological-booleans", "cost-endpoints"]
MEASUREMENTS = {"iterations", "median_total_ns", "median_ns_per_op", "checksum",
                *(f"sample_{i}_total_ns" for i in range(5))}


def read_rows(path, axis):
    with path.open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    if len(rows) != AXES[axis]:
        raise ValueError(f"{path}: expected {AXES[axis]} cases, got {len(rows)}")
    result = {}
    for row in rows:
        for field in MEASUREMENTS - {"checksum"}:
            if int(row[field]) <= 0:
                raise ValueError(f"{path}: invalid measurement {field}")
        # Include observed response/cost values in the identity so variants cannot
        # silently compare timings for different answers.
        key = tuple(sorted((k, v) for k, v in row.items() if k not in MEASUREMENTS))
        if key in result:
            raise ValueError(f"{path}: duplicate case {key}")
        result[key] = int(row["median_ns_per_op"])
    return result


def summarize(data):
    rows = []
    for (axis, key), variants in data.items():
        before = statistics.median(variants["before"])
        after = statistics.median(variants["after"])
        row = {"axis": axis, **dict(key), "before_ns": before, "after_ns": after,
               "after_over_before": after / before,
               "reduction_percent": 100 * (1 - after / before)}
        for variant, values in variants.items():
            row.update({f"{variant}_min_ns": min(values), f"{variant}_max_ns": max(values)})
        rows.append(row)
    return rows


def scaling(rows):
    groups = defaultdict(list)
    for row in rows:
        axis = row["axis"]
        if axis.endswith("schema-size") or axis.endswith("query-size"):
            groups[(axis, row["backend"], row.get("variables", "with-values"))].append(row)
    exponents = []
    for (axis, backend, variables), points in groups.items():
        dimension = "object_types" if axis.endswith("schema-size") else "query_spreads"
        xs = [math.log(int(row[dimension])) for row in points]
        for variant in ["before", "after"]:
            fit = statistics.linear_regression(xs, [math.log(row[f"{variant}_ns"]) for row in points])
            exponents.append(dict(axis=axis, backend=backend, variables=variables,
                                  variant=variant, points=len(points), exponent=fit.slope))
    return exponents


def compare(before, after, output, axes, replicates):
    snapshots = {"before": before.resolve(), "after": after.resolve()}
    metadata = {name: verify_snapshot(path) for name, path in snapshots.items()}
    require_comparable(metadata["before"], metadata["after"])
    output = new_directory(output)
    manifest = dict(started_at_utc=timestamp(), replicates=replicates, axes=axes,
                    host=host_details(), snapshots=metadata, status="running", runs=[])
    data = defaultdict(lambda: defaultdict(list))
    expected = {}
    try:
        for axis in axes:
            for replicate in range(1, replicates + 1):
                for variant, snapshot in snapshots.items():
                    verified = verify_snapshot(snapshot)
                    if verified != metadata[variant]:
                        raise ValueError(f"snapshot changed during comparison: {snapshot}")
                    label = f"{axis}-{variant}-{replicate}"
                    print(label, flush=True)
                    record = dict(axis=axis, variant=variant, replicate=replicate,
                                  started_at_utc=timestamp(), arguments=[axis], csv=f"{label}.csv")
                    write_json(output / "manifest.json", manifest)
                    with (output / record["csv"]).open("w") as stream:
                        subprocess.run([str(snapshot / "benchmark"), axis], stdout=stream, check=True)
                    values = read_rows(output / record["csv"], axis)
                    if set(values) != expected.setdefault(axis, set(values)):
                        raise ValueError(f"case/result mismatch in {label}")
                    for key, value in values.items():
                        data[(axis, key)][variant].append(value)
                    record.update(finished_at_utc=timestamp(), rows=len(values),
                                  csv_sha256=sha256(output / record["csv"]))
                    manifest["runs"].append(record)
        rows = summarize(data)
        fields = list(dict.fromkeys(key for row in rows for key in row))
        with (output / "comparison.csv").open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=fields)
            writer.writeheader()
            writer.writerows(rows)
        write_json(output / "scaling.json", scaling(rows))
        manifest["status"] = "complete"
    except BaseException:
        manifest["status"] = "failed"
        raise
    finally:
        manifest["finished_at_utc"] = timestamp()
        write_json(output / "manifest.json", manifest)
    print(f"Wrote {len(rows)} pointwise medians to {output / 'comparison.csv'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", required=True, type=Path)
    parser.add_argument("--after", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--axes", nargs="+", choices=AXES, default=DEFAULT_AXES)
    parser.add_argument("--replicates", type=int, default=3)
    args = parser.parse_args()
    if args.replicates < 3:
        parser.error("comparison requires at least three fresh processes per variant")
    if len(set(args.axes)) != len(args.axes):
        parser.error("axes must be unique")
    compare(args.before, args.after, args.output_dir, args.axes, args.replicates)


if __name__ == "__main__":
    main()
