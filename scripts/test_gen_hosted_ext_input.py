#!/usr/bin/env python3
"""Tests for the hosted `ext` benchmark input generator.

`docs/TESTING.md`'s "Input" bullet requires one file that all three arms read
verbatim, produced from a fixed integer seed by a generator committed with the
benchmark. These tests pin the properties that make the committed SHA-256
meaningful: the same seed produces the same bytes in a *separate process* (so a
hash-ordering leak cannot hide behind one interpreter's iteration order), a
different seed produces different bytes, and the serialized layout is fixed.
"""

from __future__ import annotations

import hashlib
import importlib.util
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

MODULE_PATH = Path(__file__).with_name("gen_hosted_ext_input.py")
SPEC = importlib.util.spec_from_file_location("hosted_ext_input_generator", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load the hosted ext input generator")
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


def generate_in_subprocess(output: Path, count: int, hash_seed: str) -> None:
    environment = dict(os.environ)
    environment["PYTHONHASHSEED"] = hash_seed
    subprocess.run(
        [
            sys.executable,
            "-B",
            str(MODULE_PATH),
            "--output",
            str(output),
            "--triangles",
            str(count),
        ],
        check=True,
        env=environment,
        capture_output=True,
    )


class DeterminismTest(unittest.TestCase):
    def test_two_separate_processes_agree_under_different_hash_seeds(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first.bin"
            second = root / "second.bin"

            generate_in_subprocess(first, 64, "0")
            generate_in_subprocess(second, 64, "12345")

            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_a_different_seed_produces_different_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pinned = root / "pinned.bin"
            other = root / "other.bin"

            GENERATOR.write_input(pinned, triangles=64, seed=GENERATOR.SEED)
            GENERATOR.write_input(other, triangles=64, seed=GENERATOR.SEED + 1)

            self.assertNotEqual(pinned.read_bytes(), other.read_bytes())


class LayoutTest(unittest.TestCase):
    def test_header_and_record_layout_are_fixed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.bin"
            GENERATOR.write_input(path, triangles=3, seed=GENERATOR.SEED)
            payload = path.read_bytes()

        self.assertEqual(payload[:8], GENERATOR.MAGIC)
        version, triangles, queries = struct.unpack_from("<III", payload, 8)
        self.assertEqual(version, GENERATOR.FORMAT_VERSION)
        self.assertEqual(triangles, 3)
        self.assertEqual(queries, 3)
        self.assertEqual(
            len(payload),
            GENERATOR.HEADER_SIZE + 3 * GENERATOR.TRIANGLE_SIZE + 3 * GENERATOR.QUERY_SIZE,
        )

    def test_coordinates_stay_inside_the_generated_extent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.bin"
            GENERATOR.write_input(path, triangles=128, seed=GENERATOR.SEED)
            payload = path.read_bytes()

        offset = GENERATOR.HEADER_SIZE
        values = struct.unpack_from("<768d", payload, offset)
        for value in values:
            self.assertGreaterEqual(value, 0.0)
            self.assertLess(value, GENERATOR.EXTENT)

    def test_digest_of_matches_hashlib(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.bin"
            GENERATOR.write_input(path, triangles=8, seed=GENERATOR.SEED)

            self.assertEqual(
                GENERATOR.digest_of(path),
                hashlib.sha256(path.read_bytes()).hexdigest(),
            )


class PreRegistrationTest(unittest.TestCase):
    def test_committed_seed_and_triangle_count_match_the_record(self) -> None:
        record = GENERATOR.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        self.assertEqual(record["seed"], GENERATOR.SEED)
        self.assertEqual(record["triangles"], GENERATOR.TRIANGLE_COUNT)
        self.assertEqual(record["generator_path"], "scripts/gen_hosted_ext_input.py")


if __name__ == "__main__":
    unittest.main()
