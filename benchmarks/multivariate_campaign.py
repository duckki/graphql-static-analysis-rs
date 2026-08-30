#!/usr/bin/env python3
"""Randomized fresh-process campaign for secondary cost-analysis dimensions."""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import random
import subprocess
from pathlib import Path

from campaign import provenance


BACKENDS = ("exact-case", "syntactic")
SAMPLE_COLUMNS = tuple(f"sample_{index}_total_ns" for index in range(5))
ABSTRACT_POINTS = tuple(range(8, 81, 8))
INCIDENCE_POINTS = tuple(range(1, 9))
STRUCTURE_POINTS = (1, 2, 4, 8, 16, 32)


def command_output(command: list[str]) -> str:
    return subprocess.run(command, check=True, text=True, capture_output=True).stdout


def parse_one(output: str) -> dict[str, str]:
    rows = list(csv.DictReader(output.splitlines()))
    if len(rows) != 1:
        raise RuntimeError(f"expected one result row, received {len(rows)}")
    return rows[0]


def plan(replicates: int) -> list[tuple[int, str, int, str]]:
    points = []
    for replicate in range(replicates):
        for backend in BACKENDS:
            points.extend(
                (replicate, "abstract-partition-count", value, backend)
                for value in ABSTRACT_POINTS
            )
            points.extend(
                (replicate, "unused-abstract-types", value, backend)
                for value in ABSTRACT_POINTS
            )
            points.extend(
                (replicate, "incidences-per-object", value, backend)
                for value in INCIDENCE_POINTS
            )
            points.extend(
                (replicate, "nesting-depth", value, backend)
                for value in STRUCTURE_POINTS
            )
            points.extend(
                (replicate, "response-fan-in", value, backend)
                for value in STRUCTURE_POINTS
            )
    return points


def run_point(binary: Path, dimension: str, value: int, backend: str) -> dict[str, str]:
    if dimension == "abstract-partition-count":
        command = [
            str(binary), "cost-topology-point", "1024", str(value),
            str(min(4, value)), "8", backend,
        ]
    elif dimension == "unused-abstract-types":
        command = [
            str(binary), "cost-unused-abstract-point", "1024", str(value),
            "8", "4", "8", backend,
        ]
    elif dimension == "incidences-per-object":
        command = [
            str(binary), "cost-topology-point", "1024", "80", str(value), "8", backend,
        ]
    elif dimension == "nesting-depth":
        command = [str(binary), "cost-structure-point", str(value), "1", backend]
    else:
        command = [str(binary), "cost-structure-point", "4", str(value), backend]
    return parse_one(command_output(command))


def write_csv(path: Path, rows: list[dict[str, object]]) -> None:
    fieldnames = list(dict.fromkeys(key for row in rows for key in row))
    with path.open("w", newline="") as destination:
        writer = csv.DictWriter(destination, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--replicates", type=int, default=10)
    parser.add_argument("--seed", type=int, default=20260829)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path(__file__).parent / "target/release/graphql-static-analysis-benchmark",
    )
    arguments = parser.parse_args()
    if arguments.replicates <= 0:
        parser.error("--replicates must be positive")
    binary = arguments.binary.resolve()
    if not binary.is_file():
        parser.error(f"benchmark binary does not exist: {binary}")

    randomized_plan = plan(arguments.replicates)
    random.Random(arguments.seed).shuffle(randomized_plan)
    wide: list[dict[str, object]] = []
    long: list[dict[str, object]] = []
    for run_order, (replicate, dimension, value, backend) in enumerate(randomized_plan):
        timestamp = dt.datetime.now(dt.UTC).isoformat()
        row = run_point(binary, dimension, value, backend)
        annotated: dict[str, object] = {
            "run_order": run_order,
            "replicate": replicate,
            "timestamp_utc": timestamp,
            "dimension": dimension,
            "dimension_value": value,
            **row,
        }
        wide.append(annotated)
        iterations = int(row["iterations"])
        for sample_index, column in enumerate(SAMPLE_COLUMNS):
            total_ns = int(row[column])
            long.append(
                {
                    "run_order": run_order,
                    "replicate": replicate,
                    "timestamp_utc": timestamp,
                    "dimension": dimension,
                    "dimension_value": value,
                    "backend": backend,
                    "sample_index": sample_index,
                    "iterations": iterations,
                    "total_ns": total_ns,
                    "ns_per_op": total_ns / iterations,
                }
            )

    output_dir = arguments.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    write_csv(output_dir / "multivariate-campaign-wide.csv", wide)
    write_csv(output_dir / "multivariate-campaign-long.csv", long)
    details = provenance(
        Path(__file__).resolve().parent.parent,
        arguments.seed,
        arguments.replicates,
    )
    details["dimensions"] = {
        "abstract_partition_count": ABSTRACT_POINTS,
        "unused_abstract_types": ABSTRACT_POINTS,
        "incidences_per_object": INCIDENCE_POINTS,
        "nesting_depth": STRUCTURE_POINTS,
        "response_fan_in": STRUCTURE_POINTS,
    }
    (output_dir / "provenance.json").write_text(json.dumps(details, indent=2) + "\n")


if __name__ == "__main__":
    main()
