#!/usr/bin/env python3
"""Generate the hosted `ext` benchmark's one committed input file.

`docs/TESTING.md`'s "Input" bullet requires the workload to come from a
generator committed with the benchmark, from a fixed integer seed, serialized
to one file that all three arms read verbatim, with the generator's path, the
seed and the file's SHA-256 committed before any run may be scored. This is
that generator; `scripts/bench_hosted_ext_precommit.json` is that record.

The shape is the probe D-244's Context already records publicly: a barycentric
point-in-triangle sweep over 2,000,000 triangles, one query point per triangle.
Nothing here is derived from the reference codebase's source.

Byte-reproducibility is the whole point of the committed digest, so the file
format is fixed and the generation path avoids every source of drift:

* `random.Random(seed)` and its `random()` method only -- the Mersenne Twister
  stream is stable across interpreter versions, while `uniform()`, `randrange()`
  and NumPy's generators are not contractually pinned to a byte sequence;
* little-endian IEEE-754 binary64 via a fixed `struct` format;
* records written in generation order, never from a dict or a set.

The file is large (~128 MB at the committed triangle count) and is therefore
never committed -- only its digest is. Write it outside the repository.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import random
import struct
import sys

SEED = 20260913
TRIANGLE_COUNT = 2_000_000
EXTENT = 1024.0

MAGIC = b"PYCCHEXT"
FORMAT_VERSION = 1
HEADER_FORMAT = "<III"
HEADER_SIZE = len(MAGIC) + struct.calcsize(HEADER_FORMAT)
TRIANGLE_FORMAT = "<6d"
TRIANGLE_SIZE = struct.calcsize(TRIANGLE_FORMAT)
QUERY_FORMAT = "<2d"
QUERY_SIZE = struct.calcsize(QUERY_FORMAT)

_CHUNK_TRIANGLES = 4096


def write_input(path: Path, triangles: int, seed: int) -> None:
    """Serialize `triangles` triangles and one query point each to `path`."""

    if triangles < 1:
        raise ValueError("the triangle count must be positive")
    rng = random.Random(seed)
    triangle_pack = struct.Struct(TRIANGLE_FORMAT).pack
    query_pack = struct.Struct(QUERY_FORMAT).pack
    draw = rng.random
    extent = EXTENT

    triangle_chunks: list[bytes] = []
    query_chunks: list[bytes] = []
    pending_triangles: list[bytes] = []
    pending_queries: list[bytes] = []
    for _ in range(triangles):
        coordinates = [draw() * extent for _ in range(6)]
        pending_triangles.append(triangle_pack(*coordinates))
        pending_queries.append(query_pack(draw() * extent, draw() * extent))
        if len(pending_triangles) == _CHUNK_TRIANGLES:
            triangle_chunks.append(b"".join(pending_triangles))
            query_chunks.append(b"".join(pending_queries))
            pending_triangles.clear()
            pending_queries.clear()
    if pending_triangles:
        triangle_chunks.append(b"".join(pending_triangles))
        query_chunks.append(b"".join(pending_queries))

    with path.open("wb") as handle:
        handle.write(MAGIC)
        handle.write(struct.pack(HEADER_FORMAT, FORMAT_VERSION, triangles, triangles))
        for chunk in triangle_chunks:
            handle.write(chunk)
        for chunk in query_chunks:
            handle.write(chunk)


def digest_of(path: Path) -> str:
    """Return the SHA-256 the pre-registration record commits."""

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_pre_registration(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output", type=Path, required=True, help="file to write")
    parser.add_argument(
        "--triangles",
        type=int,
        default=TRIANGLE_COUNT,
        help="triangle count; leave at the committed default for a scored run",
    )
    parser.add_argument(
        "--print-digest",
        action="store_true",
        help="print the SHA-256 of the file just written",
    )
    arguments = parser.parse_args(argv)
    try:
        write_input(arguments.output, triangles=arguments.triangles, seed=SEED)
    except ValueError as error:
        print(str(error), file=sys.stderr)
        return 2
    if arguments.print_digest:
        print(digest_of(arguments.output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
