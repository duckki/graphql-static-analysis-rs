#!/usr/bin/env python3
"""Print endpoint timings and least-squares scaling exponents for benchmark CSVs."""

import csv
import math
import pathlib
import sys


def main() -> None:
    for filename in sys.argv[1:]:
        path = pathlib.Path(filename)
        with path.open(newline="") as source:
            rows = list(csv.DictReader(source))
        x_column = "object_types" if "schema" in path.stem else "query_spreads"
        groups = {}
        for row in rows:
            groups.setdefault((row["backend"], row["variables"]), []).append(row)
        print(path)
        for key, series in groups.items():
            xs = [math.log(float(row[x_column])) for row in series]
            ys = [math.log(float(row["median_ns_per_op"])) for row in series]
            x_mean = sum(xs) / len(xs)
            y_mean = sum(ys) / len(ys)
            exponent = sum(
                (x - x_mean) * (y - y_mean) for x, y in zip(xs, ys)
            ) / sum((x - x_mean) ** 2 for x in xs)
            first_us = float(series[0]["median_ns_per_op"]) / 1_000
            last_us = float(series[-1]["median_ns_per_op"]) / 1_000
            print(f"  {'/'.join(key)}: {first_us:.1f}->{last_us:.1f} us, p={exponent:.3f}")


if __name__ == "__main__":
    main()
