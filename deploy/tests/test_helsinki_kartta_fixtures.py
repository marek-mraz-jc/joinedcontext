"""The recorded kartta.hel.fi WFS fixtures parse as GeoJSON with features in Helsinki (T-5668)."""

import json
from pathlib import Path

import pytest

KARTTA = Path(__file__).resolve().parent / "fixtures/helsinki/kartta"
# Helsinki and its sea area, generous: lon 24.5..25.5, lat 59.9..60.4.
BOX = (24.5, 59.9, 25.5, 60.4)


def points(coords):
    if isinstance(coords[0], (int, float)):
        yield coords
    else:
        for c in coords:
            yield from points(c)


@pytest.mark.parametrize("path", sorted(KARTTA.glob("*.json")), ids=lambda p: p.stem)
def test_each_fixture_is_geojson_with_a_feature_in_helsinki(path):
    doc = json.loads(path.read_text())
    assert doc["type"] == "FeatureCollection"
    assert doc["features"], f"{path.name} has no feature"
    assert len(path.read_bytes()) <= 64 * 1024
    for feature in doc["features"]:
        assert feature["type"] == "Feature"
        lon, lat = next(points(feature["geometry"]["coordinates"]))[:2]
        assert BOX[0] <= lon <= BOX[2] and BOX[1] <= lat <= BOX[3], (
            path.name,
            lon,
            lat,
        )


def test_every_named_layer_has_a_fixture():
    names = {p.stem for p in KARTTA.glob("*.json")}
    for layer in (
        "Kaupunginosajako",
        "Liikennevalot_piste",
        "Puurekisteri_piste",
        "Maisemakulttuuri_alue",
        "rantareitin_uusien_toimintojen_paikat_2025",
    ):
        assert layer in names
    assert len(names) == 96
