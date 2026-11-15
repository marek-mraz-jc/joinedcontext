// The Endpoint and the Portal's `/me` the app talks to during the browser flow: one space, a
// pipeline's station and whatever a steward adds. It is deliberately not the gateway; what it
// reproduces is what the flow needs: the roles differ by bearer token (AP-109), a viewer's write
// is refused with a problem document (GW10), and only a station without `source` may be removed
// (the grant's `q: "!source"`, R45).
import { createServer } from "node:http";

const PORT = Number(process.env.STUB_PORT ?? 4319);
const PREFIX = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki";
const ID = `${PREFIX}:kallio`;

const stations = new Map([
  [
    ID,
    {
      id: ID,
      type: "AirQualityObserved",
      name: { type: "LanguageProperty", languageMap: { fi: "Kallio", en: "Kallio" } },
      pm10: { type: "Property", value: 34.2, observedAt: "2026-09-06T10:00:00Z" },
      pm25: { type: "Property", value: 21 },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.95, 60.18] } },
      source: { type: "Property", value: "https://example.org/air-quality" },
    },
  ],
]);

const isSteward = (request) => (request.headers.authorization ?? "").includes("steward");

const send = (response, status, body) => {
  if (body === undefined) {
    response.writeHead(status).end();
    return;
  }
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    "content-type": status >= 400 ? "application/problem+json" : "application/json",
    "content-length": Buffer.byteLength(payload),
  });
  response.end(payload);
};

const forbidden = (detail) => ({ type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail });

const read = (request, then) => {
  let body = "";
  request.on("data", (chunk) => (body += chunk));
  request.on("end", () => then(JSON.parse(body || "{}")));
};

createServer((request, response) => {
  const { pathname } = new URL(request.url, `http://127.0.0.1:${PORT}`);
  const entity = /^\/ngsi-ld\/v1\/entities\/([^/]+)(\/attrs)?$/.exec(pathname);
  const id = entity ? decodeURIComponent(entity[1]) : null;

  if (pathname === "/health") {
    return send(response, 200, { ok: true });
  }
  if (pathname === "/me" && request.method === "GET") {
    const who = (request.headers.authorization ?? "").replace("Bearer token-for-", "");
    return send(response, 200, { id: `f:1:demo.${who}`, name: who, roles: [isSteward(request) ? "steward" : "viewer"] });
  }
  // One day of a station, as the temporal surface answers it (T-2925's chart): a reading every
  // three hours up to now, so the chart has a line to draw rather than an error to show.
  const temporal = /^\/ngsi-ld\/v1\/temporal\/entities\/([^/]+)$/.exec(pathname);
  if (temporal && request.method === "GET") {
    const station = stations.get(decodeURIComponent(temporal[1]));
    if (!station) {
      return send(response, 404, { type: "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound", title: "Not found", status: 404, detail: "no such station" });
    }
    const now = Date.now();
    const series = (base) =>
      Array.from({ length: 8 }, (_, step) => ({
        type: "Property",
        value: Math.round((base + (step % 3) * 2.5) * 10) / 10,
        observedAt: new Date(now - (7 - step) * 3 * 3600_000).toISOString(),
      }));
    return send(response, 200, { id: station.id, type: station.type, pm10: series(30), pm25: series(18) });
  }
  if (pathname === "/ngsi-ld/v1/entities" && request.method === "GET") {
    return send(response, 200, [...stations.values()]);
  }
  if (pathname === "/ngsi-ld/v1/entities" && request.method === "POST") {
    if (!isSteward(request)) {
      return send(response, 403, forbidden("createEntity on AirQualityObserved needs the steward role"));
    }
    return read(request, (body) => {
      stations.set(body.id, body);
      send(response, 201);
    });
  }
  if (id && entity[2] && request.method === "PATCH") {
    if (!isSteward(request)) {
      return send(response, 403, forbidden("updateAttrs on AirQualityObserved needs the steward role"));
    }
    return read(request, (body) => {
      stations.set(id, { ...stations.get(id), ...body });
      send(response, 204);
    });
  }
  if (id && !entity[2] && request.method === "DELETE") {
    if (!isSteward(request) || stations.get(id)?.source) {
      return send(response, 403, forbidden("deleteEntity is granted on records without source only"));
    }
    stations.delete(id);
    return send(response, 204);
  }
  return send(response, 404, { status: 404, detail: `no route for ${pathname}` });
}).listen(PORT, "127.0.0.1", () => {
  process.stdout.write(`stub endpoint on http://127.0.0.1:${PORT}\n`);
});
