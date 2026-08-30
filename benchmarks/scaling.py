#!/usr/bin/env python3
"""Print endpoint timings and least-squares scaling exponents for benchmark CSVs."""

import csv
import math
import pathlib
import random
import statistics
import sys


SAMPLE_COLUMNS = [f"sample_{index}_total_ns" for index in range(5)]
BOOTSTRAP_REPLICATES = 10_000


def fit_exponent(xs: list[float], ys: list[float]) -> float:
    x_mean = sum(xs) / len(xs)
    y_mean = sum(ys) / len(ys)
    return sum(
        (x - x_mean) * (y - y_mean) for x, y in zip(xs, ys)
    ) / sum((x - x_mean) ** 2 for x in xs)


def percentile(sorted_values: list[float], probability: float) -> float:
    return sorted_values[round(probability * (len(sorted_values) - 1))]


def rows_by_point(
    series: list[dict[str, str]], x_column: str
) -> dict[int, list[dict[str, str]]]:
    points: dict[int, list[dict[str, str]]] = {}
    for row in series:
        points.setdefault(int(row[x_column]), []).append(row)
    return points


def point_medians(
    series: list[dict[str, str]], x_column: str
) -> tuple[list[float], list[float]]:
    points = rows_by_point(series, x_column)
    xs = sorted(points)
    ys = [
        statistics.median(float(row["median_ns_per_op"]) for row in points[x])
        for x in xs
    ]
    return [float(x) for x in xs], ys


def exponent_interval(
    series: list[dict[str, str]], x_column: str, seed: int
) -> tuple[float, float]:
    generator = random.Random(seed)
    points = rows_by_point(series, x_column)
    point_values = sorted(points)
    xs = [math.log(float(value)) for value in point_values]
    exponents = []
    for _ in range(BOOTSTRAP_REPLICATES):
        ys = []
        for value in point_values:
            rows = points[value]
            process_samples = []
            for _ in rows:
                row = generator.choice(rows)
                sample = float(row[generator.choice(SAMPLE_COLUMNS)])
                process_samples.append(sample / float(row["iterations"]))
            ys.append(math.log(statistics.median(process_samples)))
        exponents.append(fit_exponent(xs, ys))
    exponents.sort()
    return percentile(exponents, 0.025), percentile(exponents, 0.975)


def main() -> None:
    for filename in sys.argv[1:]:
        path = pathlib.Path(filename)
        with path.open(newline="") as source:
            rows = list(csv.DictReader(source))
        groups = {}
        multivariate = bool(rows and "dimension" in rows[0])
        for row in rows:
            if multivariate:
                groups.setdefault((row["dimension"], row["backend"], ""), []).append(row)
                continue
            axis = row.get("axis")
            if not axis:
                axis = "schema-size" if "schema" in path.stem else "query-size"
            groups.setdefault(
                (axis, row["backend"], row.get("variables", "with-values")), []
            ).append(row)
        print(path)
        for group_index, (key, series) in enumerate(groups.items()):
            axis, backend, variables = key
            x_column = (
                "dimension_value"
                if multivariate
                else "object_types" if axis == "schema-size" else "query_spreads"
            )
            raw_xs, raw_ys = point_medians(series, x_column)
            xs = [math.log(value) for value in raw_xs]
            ys = [math.log(value) for value in raw_ys]
            exponent = fit_exponent(xs, ys)
            interval = exponent_interval(series, x_column, group_index)
            fitted = [
                statistics.mean(ys) + exponent * (x - statistics.mean(xs))
                for x in xs
            ]
            residual = sum((actual - predicted) ** 2 for actual, predicted in zip(ys, fitted))
            total = sum((actual - statistics.mean(ys)) ** 2 for actual in ys)
            r_squared = 1.0 - residual / total if total else 1.0
            first_us = raw_ys[0] / 1_000
            last_us = raw_ys[-1] / 1_000
            print(
                f"  {axis}/{backend}{('/' + variables) if variables else ''}: "
                f"{first_us:.1f}->{last_us:.1f} us, "
                f"p={exponent:.3f} (hierarchical bootstrap 95% CI "
                f"[{interval[0]:.3f}, {interval[1]:.3f}]), R^2={r_squared:.3f}"
            )


if __name__ == "__main__":
    main()
