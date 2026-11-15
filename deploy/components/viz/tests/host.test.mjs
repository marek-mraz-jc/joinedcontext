// T-4641: the host page reads only the declared source of the view's Endpoint, within limits.
//   node --test components/viz/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const host = createRequire(import.meta.url)("../charts/renderer/files/site/host.js");
const API = "https://api.dev.joinedcontext.com";
const SLUG = "3iasem46w7d7pxxypwdfdzheyaodv3qn";
const NOW = Date.parse("2026-10-18T16:00:00Z");
const base = `${API}/api/endpoint/${SLUG}/`;

test("the fragment names a view id, inline and a language, nothing else", () => {
  const id = "6f1c2b9e-0d0a-4c35-9a51-3a3f8f6c1d20";
  assert.deepEqual(host.parseHash(`#${id}&inline&lang=sk`), { id, inline: true, lang: "sk" });
  assert.deepEqual(host.parseHash(`#${id}`), { id, inline: false, lang: null });
  for (const bad of ["", "#", "#https://evil.example/", `#${id}x`, "#../../x"]) assert.equal(host.parseHash(bad), null);
  assert.deepEqual(host.hosts("viz.dev.joinedcontext.com"), { domain: "dev.joinedcontext.com", api: API, data: "https://opendata.dev.joinedcontext.com" });
});

test("a window of at most 31 days", () => {
  assert.equal(host.duration("PT24H"), 86400000);
  assert.equal(host.duration("P1DT12H"), 129600000);
  assert.equal(host.duration("P31D"), 31 * 86400000);
  for (const bad of ["P32D", "P", "PT", "PT0H", "24h", "", undefined, "P1Y"]) assert.equal(host.duration(bad), null, String(bad));
});

test("each source is one URL under the view's Endpoint", () => {
  assert.equal(host.firstRequest({ source: "csv", type: "WeatherObserved" }, SLUG, API, NOW).url, `${base}file.csv?type=WeatherObserved`);
  assert.equal(host.firstRequest({ source: "ogc", collection: "AirQualityObserved", filter: "pm10 > 50" }, SLUG, API, NOW).url,
    `${base}ogc/features/collections/AirQualityObserved/items?limit=1000&filter=pm10%20%3E%2050`);
  assert.equal(host.firstRequest({ source: "odata", entitySet: "WeatherObserved", $select: "name,temperature", $filter: "temperature ne null" }, SLUG, API, NOW).url,
    `${base}odata/v4/WeatherObserved?%24filter=temperature%20ne%20null&%24select=name%2Ctemperature`);
  assert.equal(host.firstRequest({ source: "temporal", type: "WeatherObserved", attrs: ["temperature"], last: "PT24H", aggrMethod: "avg", aggrPeriod: "PT1H" }, SLUG, API, NOW).url,
    `${base}ngsi-ld/v1/temporal/entities?type=WeatherObserved&attrs=temperature&timerel=after&timeAt=2026-10-17T16%3A00%3A00Z&format=aggregatedValues&aggrMethods=avg&aggrPeriodDuration=PT1H&limit=1000`);
  // T-5777: the broker's ceiling of instances per window, not its default of 100
  assert.equal(host.firstRequest({ source: "temporal", type: "WeatherObserved", attrs: ["temperature"], last: "P7D" }, SLUG, API, NOW).url,
    `${base}ngsi-ld/v1/temporal/entities?type=WeatherObserved&attrs=temperature&timerel=after&timeAt=2026-10-11T16%3A00%3A00Z&format=temporalValues&firstN=1000&limit=1000`);
});

test("nothing that could leave the Endpoint", () => {
  const bad = [
    [{ source: "csv", type: "../../x" }, SLUG],
    [{ source: "odata", entitySet: "WeatherObserved/../../../evil" }, SLUG],
    [{ source: "ogc", collection: "a?x=1" }, SLUG],
    [{ source: "url", url: "https://evil.example/" }, SLUG],
    [{ source: "temporal", type: "W", attrs: ["t"], last: "P90D" }, SLUG],
    [{ source: "temporal", type: "W", attrs: [], last: "PT1H" }, SLUG],
    [{ source: "temporal", type: "W", attrs: ["t"], last: "PT1H", aggrMethod: "eval" , aggrPeriod: "PT1H" }, SLUG],
    [{ source: "csv", type: "W" }, "https://evil.example"],
    [{ source: "csv", type: "W" }, undefined],
  ];
  for (const [source, endpoint] of bad) assert.throws(() => host.firstRequest(source, endpoint, API, NOW), JSON.stringify(source));
});

test("temporal answers become one row per instant, named in the page's language", () => {
  const request = { attrs: ["temperature"], method: "avg" };
  const rows = host.rowsFromTemporal([{ id: "urn:a", temperature: { avg: [[9.1, "2026-10-17T16:00:00Z", "2026-10-17T17:00:00Z"], [8.6, "2026-10-17T17:00:00Z", "x"]] } },
    { id: "urn:b", temperature: { type: "Property" } }], request, { "urn:a": "Donovaly" });
  assert.deepEqual(rows, [{ id: "urn:a", name: "Donovaly", observedAt: "2026-10-17T16:00:00Z", temperature: 9.1 },
    { id: "urn:a", name: "Donovaly", observedAt: "2026-10-17T17:00:00Z", temperature: 8.6 }]);
  assert.deepEqual(host.rowsFromTemporal({ error: 1 }, request, {}), []);
  assert.deepEqual(host.named([{ id: "urn:a", name: null, t: 1 }, { id: "urn:c", name: { languageMap: { en: "C", sk: "Cé" } } }], { "urn:a": "A" }, "sk"),
    [{ id: "urn:a", name: "A", t: 1 }, { id: "urn:c", name: "Cé" }]);
});

const answer = (body, status = 200) => {
  const bytes = new TextEncoder().encode(typeof body === "string" ? body : JSON.stringify(body));
  return { ok: status === 200, status, headers: { get: () => String(bytes.length) },
    body: { getReader() { let done = false; return { read: async () => (done ? { done: true } : (done = true, { done: false, value: bytes })), cancel() {} }; } } };
};
const view = (source) => ({ endpoint: SLUG, source: JSON.stringify(source) });

test("the rows of a view, read anonymously from its Endpoint alone", async () => {
  const asked = [];
  const fetcher = async (url, options) => {
    asked.push(url);
    assert.equal(options.credentials, "omit");
    if (url.includes("/odata/v4/")) return answer({ value: [{ id: "urn:a", name: null, temperature: 11.5 }] });
    return answer([{ id: "urn:a", name: "Stanica SHMÚ Donovaly" }]);
  };
  const data = await host.load(view({ source: "odata", entitySet: "WeatherObserved" }), API, "sk", fetcher, NOW);
  assert.deepEqual(data, { format: "json", rows: [{ id: "urn:a", name: "Stanica SHMÚ Donovaly", temperature: 11.5 }] });
  assert.ok(asked.every((u) => u.startsWith(base)), asked.join());
});

test("a next page off the Endpoint stops the chart, more rows than 50,000 too", async () => {
  const off = async () => answer({ value: [{ id: "urn:a" }], "@odata.nextLink": "https://evil.example/next" });
  await assert.rejects(host.load(view({ source: "odata", entitySet: "W" }), API, "en", off, NOW), /not on the Endpoint/);
  const many = async (url) => answer(url.includes("odata") ? { value: Array.from({ length: 50001 }, (_, i) => ({ i })) } : []);
  await assert.rejects(host.load(view({ source: "odata", entitySet: "W" }), API, "en", many, NOW), /50,000 rows/);
  const huge = async () => ({ ...answer("x"), headers: { get: () => String(21 * 1024 * 1024) } });
  await assert.rejects(host.load(view({ source: "csv", type: "W" }), API, "en", huge, NOW), /20 MB/);
  const refused = async () => answer({}, 403);
  await assert.rejects(host.load(view({ source: "csv", type: "W" }), API, "en", refused, NOW), /answered 403/);
  await assert.rejects(host.load(view({ source: "csv", type: "W" }), API, "en", refused, NOW), (error) => error.status === 403);
});

test("a refused read tells the reader in the page's language and keeps the status (T-5421)", () => {
  assert.equal(host.unreadable(404, "sk"), "Údaje tohto grafu teraz nie sú dostupné. (HTTP 404)");
  assert.equal(host.unreadable(403, "en"), "The data of this chart is not available now. (HTTP 403)");
  assert.equal(host.unreadable(404, "xx"), host.unreadable(404, "en"));
  assert.doesNotMatch(host.unreadable(404, "sk"), /answered/);
});

test("only a page on our own hosts may hand the host page a spec (T-4657)", () => {
  const domain = "dev.joinedcontext.com";
  assert.equal(host.ourOrigin("https://assistant.dev.joinedcontext.com", domain), true);
  assert.equal(host.ourOrigin("https://dev.joinedcontext.com", domain), true);
  for (const origin of ["null", "http://assistant.dev.joinedcontext.com", "https://evil.example", "https://dev.joinedcontext.com.evil.example",
    "https://evildev.joinedcontext.com", "https://assistant.dev.joinedcontext.com:8443", "", undefined,
    "https://x.apps.dev.joinedcontext.com", "https://a.b.apps.dev.joinedcontext.com"]) {
    assert.equal(host.ourOrigin(origin, domain), false, String(origin));
  }
});

// T-5414: a temporal read cut at the broker's page size (206) reads the rest of its window.
const cut = (body, headers = {}, status = 206) => {
  const a = answer(body);
  const all = { ...headers };
  return { ...a, ok: true, status, headers: { get: (name) => (name in all ? all[name] : name === "Content-Length" ? a.headers.get() : null) } };
};
const pm = { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "P7D" };
const series = (id, pairs) => ({ id, type: "AirQualityObserved", no2: { type: "Property", values: pairs } });
const no2 = (rows) => rows.map((r) => [r.id, r.observedAt, r.no2]);

test("a 206 window follows Link intervalafter on the Endpoint and merges one series per entity", async () => {
  const asked = [];
  const next = `/api/endpoint/${SLUG}/ngsi-ld/v1/temporal/entities?attrs=no2&format=temporalValues&limit=1000&offsetN=2&timeAt=2026-10-11T16%3A00%3A00Z&timerel=after&type=AirQualityObserved`;
  const fetcher = async (url) => {
    asked.push(url);
    if (!url.includes("/temporal/")) return answer([{ id: "urn:a", name: "Letňany" }]);
    if (!url.includes("offsetN")) {
      return cut([series("urn:a", [[30, "2026-10-16T06:00:00Z"], [29, "2026-10-21T04:00:00Z"]]), series("urn:b", [[1, "2026-10-16T06:00:00Z"], [2, "2026-10-21T08:00:00Z"]])],
        { "Content-Range": "date-time 2026-10-11T16:00:00Z-2026-10-21T08:00:00Z/*", Link: `<https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld>; rel="http://www.w3.org/ns/json-ld#context", <${next}>; rel="intervalafter";type="application/json"` });
    }
    return cut([series("urn:a", [[45, "2026-10-21T05:00:00Z"], [8, "2026-10-23T03:00:00Z"]]), series("urn:b", [[3, "2026-10-23T03:00:00Z"]])], {}, 200);
  };
  const data = await host.load(view(pm), API, "en", fetcher, NOW);
  assert.equal(asked[1], `${API}${next}`);
  assert.deepEqual(no2(data.rows), [
    ["urn:a", "2026-10-16T06:00:00Z", 30], ["urn:a", "2026-10-21T04:00:00Z", 29], ["urn:a", "2026-10-21T05:00:00Z", 45], ["urn:a", "2026-10-23T03:00:00Z", 8],
    ["urn:b", "2026-10-16T06:00:00Z", 1], ["urn:b", "2026-10-21T08:00:00Z", 2], ["urn:b", "2026-10-23T03:00:00Z", 3]]);
  assert.equal(data.rows[0].name, "Letňany");
  assert.equal(data.partial, undefined);
});

test("an attribute with several instances is one series per datasetId across windows (T-5432)", async () => {
  const soil = { source: "temporal", type: "WeatherObserved", attrs: ["soilTemperature"], last: "P7D" };
  const at = (depth, pairs) => ({ type: "Property", datasetId: `urn:ngsi-ld:Dataset:shmu:soil-depth-${depth}`, values: pairs });
  const next = `/api/endpoint/${SLUG}/ngsi-ld/v1/temporal/entities?attrs=soilTemperature&format=temporalValues&limit=1000&offsetN=2&timeAt=2026-10-16T06%3A00%3A00Z&timerel=after&type=WeatherObserved`;
  const fetcher = async (url) => {
    if (!url.includes("/temporal/")) return answer([{ id: "urn:a", name: "Banská Bystrica" }]);
    if (!url.includes("offsetN")) {
      return cut([{ id: "urn:a", type: "WeatherObserved", soilTemperature: [at("5cm", [[9.5, "2026-10-16T05:00:00Z"], [9.4, "2026-10-16T06:00:00Z"]]), at("100cm", [[13.4, "2026-10-16T06:00:00Z"]])] }],
        { "Content-Range": "date-time 2026-10-11T16:00:00Z-2026-10-16T06:00:00Z/*", Link: `<${next}>; rel="intervalafter";type="application/json"` });
    }
    return cut([{ id: "urn:a", type: "WeatherObserved", soilTemperature: [at("5cm", [[9.4, "2026-10-16T06:00:00Z"], [9.1, "2026-10-17T06:00:00Z"]])] }], {}, 200);
  };
  const data = await host.load(view(soil), API, "en", fetcher, NOW);
  assert.deepEqual(data.rows.map((r) => [r.name, r.datasetId, r.observedAt, r.soilTemperature]), [
    ["soil-depth-5cm · Banská Bystrica", "urn:ngsi-ld:Dataset:shmu:soil-depth-5cm", "2026-10-16T05:00:00Z", 9.5],
    ["soil-depth-5cm · Banská Bystrica", "urn:ngsi-ld:Dataset:shmu:soil-depth-5cm", "2026-10-16T06:00:00Z", 9.4],
    ["soil-depth-5cm · Banská Bystrica", "urn:ngsi-ld:Dataset:shmu:soil-depth-5cm", "2026-10-17T06:00:00Z", 9.1],
    ["soil-depth-100cm · Banská Bystrica", "urn:ngsi-ld:Dataset:shmu:soil-depth-100cm", "2026-10-16T06:00:00Z", 13.4]]);
});

test("a 206 with Content-Range alone continues after its end and keeps the boundary instant once", async () => {
  const asked = [];
  const fetcher = async (url) => {
    asked.push(url);
    if (!url.includes("/temporal/")) return answer([]);
    if (url.includes("timeAt=2026-10-11")) {
      return cut([series("urn:a", [[30, "2026-10-16T06:00:00Z"], [29, "2026-10-21T08:00:00Z"]])], { "Content-Range": "date-time 2026-10-11T16:00:00Z-2026-10-21T08:00:00Z/*" });
    }
    return cut([series("urn:a", [[29, "2026-10-21T08:00:00Z"], [8, "2026-10-23T03:00:00Z"]])], {}, 200);
  };
  const data = await host.load(view(pm), API, "en", fetcher, NOW);
  assert.ok(asked[1].startsWith(`${base}ngsi-ld/v1/temporal/entities?`) && asked[1].includes("timeAt=2026-10-21T08%3A00%3A00Z"), asked[1]);
  assert.deepEqual(no2(data.rows), [["urn:a", "2026-10-16T06:00:00Z", 30], ["urn:a", "2026-10-21T08:00:00Z", 29], ["urn:a", "2026-10-23T03:00:00Z", 8]]);
});

test("a 206 that never ends stops at the limits with a partial note; a next window off the Endpoint stops the chart", async () => {
  let n = 0;
  const endless = async (url) => {
    if (!url.includes("/temporal/")) return answer([]);
    n += 1;
    const pairs = Array.from({ length: 1000 }, (_, i) => [i, new Date(Date.parse("2026-10-12T00:00:00Z") + (n * 1000 + i) * 1000).toISOString()]);
    return cut(Array.from({ length: 12 }, (_, i) => series(`urn:${i}`, pairs)), { Link: `<${base}ngsi-ld/v1/temporal/entities?offsetN=${n * 1000}&type=AirQualityObserved>; rel="intervalafter"` });
  };
  const rows = await host.load(view(pm), API, "sk", endless, NOW, Date.now() + 60000);
  assert.ok(rows.rows.length <= 50000 && rows.rows.length >= 48000, String(rows.rows.length));
  assert.match(rows.partial, /^Graf je neúplný: zobrazuje údaje do 2026-10-12 0\d:\d\d UTC/);
  n = 0;
  const late = await host.load(view(pm), API, "en", endless, NOW, Date.now() - 1);
  assert.equal(n, 1);
  assert.match(late.partial, /^Partial chart/);
  const looping = async (url) => (url.includes("/temporal/") ? cut([series("urn:a", [[1, "2026-10-20T00:00:00Z"]])], { Link: `<${url}>; rel="intervalafter"` }) : answer([]));
  assert.match((await host.load(view(pm), API, "en", looping, NOW)).partial, /up to 2026-10-20 00:00 UTC/);
  const off = async (url) => (url.includes("/temporal/") ? cut([series("urn:a", [[1, "2026-10-20T00:00:00Z"]])], { Link: '<https://evil.example/t>; rel="intervalafter"' }) : answer([]));
  await assert.rejects(host.load(view(pm), API, "en", off, NOW), /not on the Endpoint/);
  const first = async () => cut(Array.from({ length: 51 }, (_, i) => series(`urn:${i}`, Array.from({ length: 1000 }, (_, j) => [j, "x"]))));
  await assert.rejects(host.load(view(pm), API, "en", first, NOW), /50,000 rows/);
});

// T-6170: the edge refuses a client past 10 history reads at once; a dataset page of six charts
// lost one to a 429 the browser could not read, and the chart stayed blank.
const refusal = (retryAfter) => ({ ok: false, status: 429, headers: { get: (name) => (name === "Retry-After" ? retryAfter : null) } });
const temp = { source: "temporal", type: "WeatherObserved", attrs: ["temperature"], last: "P1D" };
const reading = [{ id: "urn:a", type: "WeatherObserved", temperature: { type: "Property", values: [[9.1, "2026-10-18T10:00:00Z"]] } }];

test("a 429 or a network error is tried once more, a second refusal reaches the reader", async () => {
  for (const first of [() => refusal("0"), () => { throw new TypeError("Failed to fetch"); }]) {
    let calls = 0;
    const fetcher = async (url) => {
      if (!url.includes("/temporal/")) return answer([]);
      calls += 1;
      return calls === 1 ? first() : answer(reading);
    };
    const started = Date.now();
    const data = await host.load(view(temp), API, "en", fetcher, NOW);
    assert.equal(calls, 2);
    assert.deepEqual(data.rows.map((r) => r.temperature), [9.1]);
    assert.ok(Date.now() - started >= 1000, "it waits before the second try");
  }
  let calls = 0;
  const always = async (url) => (url.includes("/temporal/") ? (calls += 1, refusal("1")) : answer([]));
  await assert.rejects(host.load(view(temp), API, "en", always, NOW), (error) => error.status === 429);
  assert.equal(calls, 2, "one retry, not a loop");
  const aborted = async () => { throw Object.assign(new Error("aborted"), { name: "AbortError" }); };
  await assert.rejects(host.load(view(temp), API, "en", aborted, NOW), (error) => error.name === "AbortError");
});

// Web Locks as a browser grants them: one holder per name, `ifAvailable` answers null at once.
const webLocks = () => {
  const lines = new Map(); // a held name -> the requests waiting for it
  const release = (name) => { const next = lines.get(name).shift(); if (next) next(); else lines.delete(name); };
  const request = (name, options, run) => {
    if (typeof options === "function") return request(name, {}, options);
    if (lines.has(name) && options.ifAvailable) return Promise.resolve().then(() => run(null));
    const granted = lines.has(name) ? new Promise((resolve) => lines.get(name).push(resolve)) : (lines.set(name, []), Promise.resolve());
    return granted.then(() => run({ name })).finally(() => release(name));
  };
  return { request };
};

for (const [label, locks] of [["in one frame", null], ["across frames (Web Locks)", webLocks()]]) {
  test(`six charts read at most four at once, ${label}`, async (t) => {
    if (locks) {
      const before = Object.getOwnPropertyDescriptor(globalThis, "navigator");
      Object.defineProperty(globalThis, "navigator", { value: { locks }, configurable: true });
      t.after(() => (before ? Object.defineProperty(globalThis, "navigator", before) : delete globalThis.navigator));
    }
    let open = 0;
    let most = 0;
    const fetcher = async (url) => {
      open += 1;
      most = Math.max(most, open);
      await new Promise((resolve) => setTimeout(resolve, 30));
      open -= 1;
      return answer(url.includes("/temporal/") ? reading : []);
    };
    const charts = await Promise.all(Array.from({ length: 6 }, () => host.load(view(temp), API, "en", fetcher, NOW)));
    assert.equal(charts.length, 6);
    assert.ok(charts.every((c) => c.rows.length === 1));
    assert.equal(most, 4);
  });
}
