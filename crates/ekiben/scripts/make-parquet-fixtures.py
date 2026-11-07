#!/usr/bin/env python3
"""Writes the synthetic Parquet edge-case fixtures of crates/ekiben-parquet/tests/parity (pyarrow, deterministic).

The real file beside them (eea-sk0020a-pm10.parquet) is the EEA E2a file the Žilina PM10 seed reads, saved 2026-11-07
from https://eeadmz1batchservice02.blob.core.windows.net/airquality-p/SK/SPO-SK0020A_00005_100.parquet.
"""
import decimal
import sys
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

out = Path(sys.argv[1] if len(sys.argv) > 1 else "crates/ekiben-parquet/tests/parity")
D = decimal.Decimal

scalars = pa.table({
    "b": pa.array([True, False, None], pa.bool_()),
    "i8": pa.array([-1, 127, None], pa.int8()),
    "u16": pa.array([0, 65535, None], pa.uint16()),
    "u32": pa.array([0, 4294967295, None], pa.uint32()),
    "i64": pa.array([-9223372036854775808, 9223372036854775807, None], pa.int64()),
    "u64": pa.array([0, 18446744073709551615, None], pa.uint64()),
    "f32": pa.array([0.1, -1.5, None], pa.float32()),
    "f64": pa.array([0.1, 1e21, None], pa.float64()),
    "s": pa.array(["žltá \"q\" <&>", "", None], pa.string()),
    "bin": pa.array([b"\x00\xff\xfe", b"plain", None], pa.binary()),
    "fixed": pa.array([b"abcd", b"\x00\x01\x02\x03", None], pa.binary(4)),
    "dec": pa.array([D("-1.500000000000000000"), D("0.000010000000000000"), None], pa.decimal128(38, 18)),
    "dec_big": pa.array([D("1234567.89"), D("-0.01"), None], pa.decimal128(20, 2)),
    "dec_i32": pa.array([D("12.34"), D("-0.05"), None], pa.decimal128(5, 2)),
    "dec_i64": pa.array([D("123456.789"), D("-0.001"), None], pa.decimal128(15, 3)),
    "date": pa.array([0, 19000, None], pa.date32()),
    "ts_ms": pa.array([0, 1700000000000, None], pa.timestamp("ms")),
})
pq.write_table(scalars, out / "scalars.parquet", compression="snappy", store_decimal_as_integer=False)
pq.write_table(scalars.select(["dec_i32", "dec_i64"]), out / "decimal-ints.parquet", store_decimal_as_integer=True)

int96 = pa.table({"t": pa.array([0, 1700000000123456789, None], pa.timestamp("ns"))})
pq.write_table(int96, out / "int96.parquet", use_deprecated_int96_timestamps=True)

nested = pa.table({
    "id": pa.array([1, 2, 3], pa.int32()),
    "point": pa.array([{"x": 1.5, "y": None}, None, {"x": 0.0, "y": "a"}],
                      pa.struct([("x", pa.float64()), ("y", pa.string())])),
    "tags": pa.array([["a", "b"], [], None], pa.list_(pa.string())),
    "maybe": pa.array([[1, None, 3], None, [None]], pa.list_(pa.int64())),
    "attrs": pa.array([[("k", 1), ("j", 2)], [], None], pa.map_(pa.string(), pa.int32())),
    "rows": pa.array([[{"v": 1}, {"v": None}], None, []], pa.list_(pa.struct([("v", pa.int32())]))),
    "grid": pa.array([[[1, 2], [3]], [[]], None], pa.list_(pa.list_(pa.int32()))),
})
pq.write_table(nested, out / "nested.parquet")

empty = pa.table({"a": pa.array([], pa.int32()), "s": pa.array([], pa.string())})
pq.write_table(empty, out / "empty.parquet")

for codec in ["gzip", "zstd", "lz4", "brotli", "none"]:
    t = pa.table({"n": pa.array(list(range(300)), pa.int64()), "s": pa.array([f"row {i}" for i in range(300)])})
    pq.write_table(t, out / f"codec-{codec}.parquet", compression=codec, row_group_size=100)

good = (out / "empty.parquet").read_bytes()
(out / "corrupt-truncated.parquet").write_bytes(good[: len(good) // 2])
(out / "corrupt-not-parquet.parquet").write_bytes(b"this is not a parquet file, only text\n")
(out / "corrupt-empty.parquet").write_bytes(b"")
