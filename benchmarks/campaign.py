#!/usr/bin/env python3
"""Run a randomized fresh-process IBM-cost benchmark campaign.

The wide CSV keeps every process result. The long CSV expands its five timed samples
and records run order, process replicate, and UTC timestamp for bootstrap analysis.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import platform
import random
import re
import subprocess
from pathlib import Path


BACKENDS = ("exact-case", "syntactic")
SCHEMA_POINTS = tuple((scale * 1024, 8, "schema-size") for scale in range(1, 11))
QUERY_POINTS = tuple((1024, scale * 8, "query-size") for scale in range(1, 11))
SAMPLE_COLUMNS = tuple(f"sample_{index}_total_ns" for index in range(5))


def command_output(*command: str) -> str:
    return subprocess.run(command, check=True, text=True, capture_output=True).stdout.strip()


def provenance(repository: Path, seed: int, replicates: int) -> dict[str, object]:
    model_source = repository / "fuzz/src/tree_summary/input.rs"
    model_match = re.search(
        r'LEAN_MODEL_COMMIT: &str = "([0-9a-f]{40})"',
        model_source.read_text(),
    )
    return {
        "created_at_utc": dt.datetime.now(dt.UTC).isoformat(),
        "seed": seed,
        "replicates": replicates,
        "git_revision": command_output("git", "-C", str(repository), "rev-parse", "HEAD"),
        "git_status": command_output("git", "-C", str(repository), "status", "--short"),
        "lean_model_revision": model_match.group(1) if model_match else None,
        "rustc": command_output("rustc", "-Vv"),
        "cargo": command_output("cargo", "-V"),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor(),
    }


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

    repository = Path(__file__).resolve().parent.parent
    output_dir = arguments.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    plan = [
        (replicate, object_count, query_spreads, axis, backend)
        for replicate in range(arguments.replicates)
        for object_count, query_spreads, axis in SCHEMA_POINTS + QUERY_POINTS
        for backend in BACKENDS
    ]
    random.Random(arguments.seed).shuffle(plan)

    wide_rows: list[dict[str, str | int]] = []
    long_rows: list[dict[str, str | int | float]] = []
    for run_order, (replicate, object_count, query_spreads, axis, backend) in enumerate(plan):
        timestamp = dt.datetime.now(dt.UTC).isoformat()
        output = command_output(
            str(binary),
            "cost-point",
            str(object_count),
            str(query_spreads),
            backend,
        )
        rows = list(csv.DictReader(output.splitlines()))
        if len(rows) != 1:
            raise RuntimeError(f"expected one benchmark row, received {len(rows)}")
        row = rows[0]
        annotated = {
            "run_order": run_order,
            "replicate": replicate,
            "timestamp_utc": timestamp,
            "axis": axis,
            **row,
        }
        wide_rows.append(annotated)
        iterations = int(row["iterations"])
        for sample_index, column in enumerate(SAMPLE_COLUMNS):
            total_ns = int(row[column])
            long_rows.append(
                {
                    "run_order": run_order,
                    "replicate": replicate,
                    "timestamp_utc": timestamp,
                    "axis": axis,
                    "backend": backend,
                    "object_types": object_count,
                    "abstract_types": row["abstract_types"],
                    "incidences_per_object": row["incidences_per_object"],
                    "query_spreads": query_spreads,
                    "type_cost": row["type_cost"],
                    "field_cost": row["field_cost"],
                    "sample_index": sample_index,
                    "iterations": iterations,
                    "total_ns": total_ns,
                    "ns_per_op": total_ns / iterations,
                }
            )

    def write_csv(path: Path, rows: list[dict[str, object]]) -> None:
        with path.open("w", newline="") as destination:
            writer = csv.DictWriter(destination, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)

    write_csv(output_dir / "cost-campaign-wide.csv", wide_rows)
    write_csv(output_dir / "cost-campaign-long.csv", long_rows)
    (output_dir / "provenance.json").write_text(
        json.dumps(provenance(repository, arguments.seed, arguments.replicates), indent=2)
        + "\n"
    )


if __name__ == "__main__":
    main()
