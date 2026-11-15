// T-5189: the vector tiles the route-stub journeys serve for the Endpoint's OGC API Tiles.
// A Mapbox Vector Tile 2.1 of the points in tile z/x/y, as Antares tiles.rs writes it: one layer
// named after the collection, each feature's URN under `id`, at most 10 000 features.
export function mvt(layer, features, z, x, y) {
  const varint = (out, v) => { while (v > 127) { out.push((v & 127) | 128); v = Math.floor(v / 128); } out.push(v); };
  const bytes = (out, field, b) => { varint(out, field * 8 + 2); varint(out, b.length); for (const x of b) out.push(x); };
  const zz = (n) => (n << 1) ^ (n >> 31);
  const n = 2 ** z;
  const keys = ["id", "name"];
  const values = [];
  const body = [];
  varint(body, 15 * 8); varint(body, 2);
  bytes(body, 1, [...Buffer.from(layer)]);
  let count = 0;
  for (const f of features) {
    const [lon, lat] = f.geometry.coordinates;
    const r = (lat * Math.PI) / 180;
    const px = Math.round((((lon + 180) / 360) * n - x) * 4096);
    const py = Math.round((((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n - y) * 4096);
    if (px < 0 || px >= 4096 || py < 0 || py >= 4096 || count >= 10000) continue;
    count += 1;
    const tags = [];
    for (const [k, v] of [[0, f.id], [1, (f.properties || {}).name]]) if (typeof v === "string") { tags.push(k, values.length); values.push(v); }
    const feature = [];
    const packed = (field, list) => { const p = []; list.forEach((v) => varint(p, v)); bytes(feature, field, p); };
    packed(2, tags);
    varint(feature, 3 * 8); varint(feature, 1);
    packed(4, [9, zz(px), zz(py)]);
    bytes(body, 2, feature);
  }
  for (const k of keys) bytes(body, 3, [...Buffer.from(k)]);
  for (const v of values) { const value = []; bytes(value, 1, [...Buffer.from(v)]); bytes(body, 4, value); }
  varint(body, 5 * 8); varint(body, 4096);
  const tile = [];
  bytes(tile, 3, body);
  return count ? Buffer.from(tile) : null;
}
