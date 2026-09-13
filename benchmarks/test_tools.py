"""Integrity checks for benchmark provenance and statistical/profile aggregation."""

import copy
import csv
import tempfile
import unittest
from pathlib import Path

from artifacts import host_details, require_comparable, sha256, verify_snapshot, write_json
from compare import read_rows, scaling, summarize
from profile_estimate import parse_sample


class MeasurementToolsTests(unittest.TestCase):
    def test_snapshot_detects_replaced_binary_and_edited_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "source/src").mkdir(parents=True)
            source = root / "source/src/lib.rs"
            source.write_text("original source")
            binary = root / "benchmark"
            binary.write_bytes(b"original binary")
            metadata = dict(format=1, binary_sha256=sha256(binary),
                            source_sha256={"src/lib.rs": sha256(source)})
            write_json(root / "manifest.json", metadata)
            self.assertEqual(verify_snapshot(root), metadata)
            binary.write_bytes(b"replaced binary")
            with self.assertRaisesRegex(ValueError, "binary hash mismatch"):
                verify_snapshot(root)
            binary.write_bytes(b"original binary")
            source.write_text("edited source")
            with self.assertRaisesRegex(ValueError, "source hash mismatch"):
                verify_snapshot(root)

    def test_comparison_allows_engine_changes_but_rejects_changed_harness(self):
        before = dict(host=host_details(), rustc="rustc", cargo="cargo", build_environment={},
                      source_sha256={"src/lib.rs": "old", "benchmarks/src/main.rs": "harness"})
        after = copy.deepcopy(before)
        after["source_sha256"]["src/lib.rs"] = "new"
        require_comparable(before, after)
        after["source_sha256"]["benchmarks/src/main.rs"] = "different"
        with self.assertRaisesRegex(ValueError, "benchmark source"):
            require_comparable(before, after)

    def test_pointwise_medians_resist_one_process_outlier(self):
        key = (("backend", "exact-case"), ("variables", "with-values"))
        rows = summarize({("endpoints", key): {"before": [100, 110, 10000], "after": [50, 55, 60]}})
        self.assertEqual(rows[0]["before_ns"], 110)
        self.assertEqual(rows[0]["after_ns"], 55)
        self.assertEqual(rows[0]["reduction_percent"], 50)
        self.assertEqual(rows[0]["before_max_ns"], 10000)

    def test_scaling_keeps_axes_and_variants_separate(self):
        rows = [dict(axis="schema-size", backend="exact-case", variables="with-values",
                     object_types=x, before_ns=x ** 2, after_ns=x) for x in [1, 2, 4, 8]]
        fits = scaling(rows)
        self.assertAlmostEqual(fits[0]["exponent"], 2)
        self.assertAlmostEqual(fits[1]["exponent"], 1)
        self.assertEqual(scaling([{**rows[0], "axis": "endpoints"}]), [])

    def test_csv_rejects_missing_and_duplicate_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rows.csv"
            row = dict(backend="exact-case", variables="with-values", object_types=1024,
                       iterations=1, median_total_ns=1, median_ns_per_op=1, checksum=1,
                       **{f"sample_{i}_total_ns": 1 for i in range(5)})
            with path.open("w", newline="") as stream:
                writer = csv.DictWriter(stream, fieldnames=list(row))
                writer.writeheader()
                writer.writerow(row)
            with self.assertRaisesRegex(ValueError, "expected 12 cases"):
                read_rows(path, "endpoints")
            with path.open("a", newline="") as stream:
                csv.DictWriter(stream, fieldnames=list(row)).writerows([row] * 11)
            with self.assertRaisesRegex(ValueError, "duplicate case"):
                read_rows(path, "endpoints")

    def test_sample_excludes_startup_and_does_not_double_count_recursion(self):
        sample = """Call graph:
    12 main
      2 parsing
      10 MaxResponseSizeEstimator::estimate
        8 condition_tree::extract
          5 condition_tree::recursive_extract
            3 allocation (in libsystem_malloc.dylib)
        2 finish
Total number in stack
"""
        totals, _ = parse_sample(sample)
        self.assertEqual(totals["estimate"], 10)
        self.assertEqual(totals["extraction_percent"], 80)
        self.assertEqual(totals["allocator_percent"], 30)
        with self.assertRaisesRegex(ValueError, "no identifiable estimate stacks"):
            parse_sample("Call graph:\n    12 parsing\nTotal number in stack\n")


if __name__ == "__main__":
    unittest.main()
