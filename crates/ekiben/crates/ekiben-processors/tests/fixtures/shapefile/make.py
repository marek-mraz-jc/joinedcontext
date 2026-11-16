#!/usr/bin/env python3
"""Writes helsinki-3879-{pisteet,viivat,alueet}.zip, one shapefile layer each in EPSG:3879 (GK25FIN, Helsinki), and expected.json,
its records as WGS 84 GeoJSON Features reprojected by PROJ (pyproj), for shapefile_tests.rs.

    python3 make.py    (needs pyproj; rerun only to change the fixture)

The .shp, .shx and .dbf are written by hand from the ESRI Shapefile Technical Description, so
the fixture does not come from the code it tests. The .prj is PROJ's own ESRI WKT of EPSG:3879.
"""
import io, json, os, struct, zipfile
from pyproj import CRS, Transformer

HERE = os.path.dirname(os.path.abspath(__file__))
# Outer rings clockwise, holes counterclockwise, as the format requires.
square = lambda x, y, d: [(x, y), (x, y + d), (x + d, y + d), (x + d, y), (x, y)]
hole = lambda x, y, d: list(reversed(square(x, y, d)))
RECORDS = [
    # (shape type, parts, properties)
    (1, [[(25496500.0, 6672600.0)]], {"NIMI": "Kauppatori", "LUKU": 12, "OSUUS": 0.25, "AUKI": True, "PVM": "20261115"}),
    (3, [[(25494000.0, 6676000.0), (25495000.0, 6676500.0), (25496000.0, 6677500.0)]], {"NIMI": "Töölönlahti", "LUKU": None, "OSUUS": None, "AUKI": False, "PVM": ""}),
    (5, [square(25495000.0, 6674000.0, 1000.0), hole(25495200.0, 6674200.0, 200.0)], {"NIMI": "Kamppi–Etu-Töölö", "LUKU": -3, "OSUUS": 1.5, "AUKI": None, "PVM": "20250101"}),
    (5, [square(25490000.0, 6670000.0, 500.0), square(25500000.0, 6680000.0, 300.0)], {"NIMI": "Saaret", "LUKU": 0, "OSUUS": 0.0, "AUKI": True, "PVM": "19991231"}),
    (0, [], {"NIMI": "Ei sijaintia", "LUKU": 7, "OSUUS": None, "AUKI": None, "PVM": ""}),
    (1, [[(25497000.0, 6673000.0)]], {"NIMI": "Poistettu", "LUKU": 1, "OSUUS": None, "AUKI": None, "PVM": "", "_deleted": True}),
]
FIELDS = [("NIMI", "C", 40, 0), ("LUKU", "N", 10, 0), ("OSUUS", "N", 12, 4), ("AUKI", "L", 1, 0), ("PVM", "D", 8, 0)]

def shape(kind, parts):
    if kind == 0:
        return struct.pack("<i", 0)
    pts = [p for part in parts for p in part]
    if kind == 1:
        return struct.pack("<idd", 1, *pts[0])
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    out = struct.pack("<i4dii", kind, min(xs), min(ys), max(xs), max(ys), len(parts), len(pts))
    start = 0
    for part in parts:
        out += struct.pack("<i", start)
        start += len(part)
    return out + b"".join(struct.pack("<dd", *p) for p in pts)

def layer(kind_filter):
    """One layer holding the records whose shape type is in kind_filter (or null)."""
    rows = [r for r in RECORDS if r[0] in kind_filter or r[0] == 0]
    recs, index, offset = b"", b"", 50
    for n, (kind, parts, _) in enumerate(rows, 1):
        content = shape(kind, parts)
        index += struct.pack(">ii", offset, len(content) // 2)
        recs += struct.pack(">ii", n, len(content) // 2) + content
        offset += 4 + len(content) // 2
    pts = [p for _, parts, _ in rows for part in parts for p in part]
    bbox = (min(p[0] for p in pts), min(p[1] for p in pts), max(p[0] for p in pts), max(p[1] for p in pts))
    ftype = [k for k in kind_filter][0]
    head = lambda length: struct.pack(">7i", 9994, 0, 0, 0, 0, 0, length // 2) + struct.pack("<ii8d", 1000, ftype, *bbox, 0, 0, 0, 0)
    shp = head(100 + len(recs)) + recs
    shx = head(100 + len(index)) + index
    return rows, shp, shx

def dbf(rows):
    rec_len = 1 + sum(f[2] for f in FIELDS)
    hdr_len = 32 + 32 * len(FIELDS) + 1
    out = struct.pack("<BBBBIHH20x", 3, 126, 11, 15, len(rows), hdr_len, rec_len)
    out = out[:29] + b"\x00" + out[30:]
    for name, kind, length, dec in FIELDS:
        out += name.encode().ljust(11, b"\x00") + kind.encode() + b"\x00" * 4 + bytes([length, dec]) + b"\x00" * 14
    out += b"\x0d"
    for _, _, props in rows:
        rec = b"*" if props.get("_deleted") else b" "
        for name, kind, length, dec in FIELDS:
            v = props[name]
            if kind == "C":
                b = v.encode("utf-8").ljust(length, b" ")
            elif kind == "N":
                b = (b"" if v is None else (f"{v:.{dec}f}" if dec else str(v)).encode()).rjust(length, b" ")
            elif kind == "L":
                b = {True: b"T", False: b"F", None: b"?"}[v]
            else:
                b = v.encode().ljust(length, b" ")
            assert len(b) == length, (name, v)
            rec += b
        out += rec
    return out + b"\x1a"

def main():
    to_wgs = Transformer.from_crs(3879, 4326, always_xy=True)
    prj = CRS.from_epsg(3879).to_wkt("WKT1_ESRI")
    expected = {}
    for stem, kind in [("pisteet", 1), ("viivat", 3), ("alueet", 5)]:
        rows, shp, shx = layer({kind})
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
            for ext, data in [("shp", shp), ("shx", shx), ("dbf", dbf(rows)), ("prj", prj), ("cpg", "UTF-8")]:
                z.writestr(f"kohteet/{stem}.{ext}", data)
            z.writestr("kohteet/LUEMINUT.txt", "Helsinki test layer, EPSG:3879\n")
        name = f"helsinki-3879-{stem}.zip"
        open(os.path.join(HERE, name), "wb").write(buf.getvalue())
        expected[name] = [
            {"kind": k, "parts": [[list(to_wgs.transform(x, y)) for x, y in part] for part in parts],
             "properties": {f: v for f, v in props.items() if not f.startswith("_")}}
            for k, parts, props in rows if not props.get("_deleted")
        ]
    with open(os.path.join(HERE, "expected.json"), "w") as f:
        json.dump({"proj": __import__("pyproj").proj_version_str, "layers": expected}, f, indent=1, ensure_ascii=False)
        f.write("\n")

main()
