# kartta.hel.fi WFS fixtures

Recorded 2026-10-26T14:55Z by the deployer (T-5668) from
`https://kartta.hel.fi/ws/geoserver/avoindata/wfs`: one GetFeature per layer, WFS 2.0.0,
`count=20`, `outputFormat=application/json`, `srsName=EPSG:4326`.

Each file keeps the server's FeatureCollection with coordinates rounded to 6 decimals and its
features sorted by size, smallest first, cut so the file stays under 64 KiB (at least one
feature). `numberReturned` counts the features kept; `numberMatched` is the server's count.

Two layers answered with no features and have no fixture: `LTJ_avoin_arvo_lahokaviosammal_tukialueet`
and `LTJ_avoin_rauhoitettu_Natura_viivamaiset`.

Tasks: T-5256 (Kaupunginosajako), T-5286 (traffic layers), T-5378 (water, trees, LTJ_avoin_*,
Meluselvitys_*, VISTRA), T-5663 rows 66, 74, 80 (Maalinnoitus_*, Maisemakulttuuri_*,
rantareitin_uusien_toimintojen_paikat_2025).
