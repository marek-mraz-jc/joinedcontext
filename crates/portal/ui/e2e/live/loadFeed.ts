/**
 * The Load journey's feed and mapping, shared by every door that plays it (T-0630, T-1595): the
 * HSL city bikes' free-bike status as GBFS, one document in, one Vehicle per bike out.
 */
export const FEED = "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/free_bike_status.json";

// One GBFS document in, one Vehicle per bike out (PL-48: the array is split by the runner).
export const MAPPING = [
  'let domain = env("JC_ORG_DOMAIN")',
  'let seen = this.last_updated.number().ts_format("2006-01-02T15:04:05Z")',
  "root = this.data.bikes.map_each(b -> {",
  '  "id": "urn:ngsi-ld:Vehicle:%v:helsinki:%v".format($domain, b.bike_id),',
  '  "type": "Vehicle",',
  '  "vehicleType": { "type": "Property", "value": "bicycle" },',
  '  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [b.lon, b.lat] } },',
  '  "serviceStatus": { "type": "Property", "value": if b.is_disabled { "outOfService" } else if b.is_reserved { "reserved" } else { "available" }, "observedAt": $seen },',
  '  "dateObserved": { "type": "Property", "value": $seen }',
  "})",
].join("\n");
