// T-5059, T-5082: the explorer page's pure logic under node: the hash (type, query and order of a
// shared view), the map's bbox, the query's API addresses and how a value reads in the map's details.
// The table, its filters and its order are the Portal's Endpoint data view, tested in the Portal.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const x = require("../charts/renderer/files/site/explore.js");

test("the hash names one Endpoint and its options; anything else is dropped", () => {
  assert.deepEqual(x.parseHash("#scsd2eehkx42n53z2zyd6vshfh7s7irf&lang=sk&type=Bike&inline&theme=dark"),
    { slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", lang: "sk", type: "Bike", inline: true, theme: "dark", q: null, sort: null, dataset: null, view: null, all: false, geo: null, color: null, pop: null, cluster: null });
  assert.equal(x.parseHash("#../evil"), null);
  assert.equal(x.parseHash(""), null);
  const odd = x.parseHash("#scsd2eehkx42n53z2zyd6vshfh7s7irf&type=a;b&lang=english&theme=red");
  assert.equal(odd.type, null);
  assert.equal(odd.lang, null);
  assert.equal(odd.theme, null);
});

test("a shared view's query and order come back from the hash they were written to", () => {
  const view = { slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", inline: true, all: false, lang: "sk", theme: null, type: "Bike", q: 'name~=".*ivo.*";bikes>=5', sort: "bikes;desc", dataset: null, view: null, geo: null, color: null, pop: null, cluster: null };
  assert.deepEqual(x.parseHash(x.hashOf(view)), view);
  assert.equal(x.hashOf({ slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf" }), "#scsd2eehkx42n53z2zyd6vshfh7s7irf");
  const bad = x.parseHash("#scsd2eehkx42n53z2zyd6vshfh7s7irf&sort=a;sideways&q=%E0%A4%A&type=Bike");
  assert.equal(bad.sort, null);
  assert.equal(bad.q, null); // a broken escape is dropped, not thrown
  assert.equal(bad.type, "Bike");
  assert.equal(x.parseHash("#scsd2eehkx42n53z2zyd6vshfh7s7irf&q=" + "a".repeat(2001)).q, null);
  assert.equal(x.parseHash("#scsd2eehkx42n53z2zyd6vshfh7s7irf&sort=id").sort, "id");
});

test("the API base comes from the page's own host", () => {
  assert.equal(x.base("viz.dev.joinedcontext.com", "abc"), "https://api.dev.joinedcontext.com/api/endpoint/abc/");
});

test("the map's view becomes a closed within-polygon, clamped to the globe", () => {
  const b = x.bboxQuery([-200, -95, 24.95123456, 60.1]);
  assert.equal(b.georel, "within");
  assert.equal(b.coordinates, JSON.stringify([[[-180, -90], [24.951235, -90], [24.951235, 60.1], [-180, 60.1], [-180, -90]]]));
});

test("links stay under the Endpoint and carry the query", () => {
  const api = "https://api.dev.joinedcontext.com/api/endpoint/abc/";
  const l = x.links(api, "Bike", 'status=="x"', "bikes;desc", "urn:ngsi-ld:Bike:1");
  assert.ok(l.every((k) => k.href.startsWith(api)));
  assert.equal(l[0].href, api + "ngsi-ld/v1/entities/urn%3Angsi-ld%3ABike%3A1");
  assert.ok(l[1].href.includes("q=status%3D%3D%22x%22") && l[1].href.includes("orderBy=bikes%3Bdesc"));
  // The CSV of exactly this view: its query and its order.
  assert.equal(l[4].href, api + "file.csv?type=Bike&q=status%3D%3D%22x%22&orderBy=bikes%3Bdesc");
  assert.equal(l[5].href, api + "ogc/features/collections/Bike/tiles/WebMercatorQuad");
  assert.equal(x.links(api, "Bike", "", null).length, 5);
});

test("cells read as text: numbers by locale, links only for http(s), no raw JSON", () => {
  const t = { yes: "áno", no: "nie", lang: "sk", locale: "sk-SK" };
  assert.equal(x.show(1330091, t).text.replace(/\s/g, " "), "1 330 091");
  assert.equal(x.show(true, t).text, "áno");
  assert.equal(x.show(null, t).text, "–");
  assert.deepEqual(x.show("https://hsl.fi/a", t), { text: "https://hsl.fi/a", href: "https://hsl.fi/a" });
  assert.equal(x.show("javascript:alert(1)", t).href, undefined);
  assert.equal(x.show({ type: "Polygon", coordinates: [[[17.1, 48.1], [17.2, 48.1], [17.1, 48.1]]] }, t).text, "Polygon 17.10000, 48.10000");
  assert.equal(x.show(["a", 2], t).text, "a; 2");
  assert.equal(x.show({ languageMap: { fi: "Kaivopuisto", sk: "Park" } }, t).text, "Park");
  assert.equal(x.show({ streetAddress: "Hlavná 1", postalCode: "97401" }, t).text, "streetAddress: Hlavná 1, postalCode: 97401");
});

test("both languages name every string", () => {
  const keys = (o, p = "") => Object.entries(o).flatMap(([k, v]) => (typeof v === "object" ? keys(v, p + k + ".") : [p + k])).sort();
  assert.deepEqual(keys(x.TEXT.sk), keys(x.TEXT.en));
});

test("a timestamp reads as a date in the page's language, in UTC", () => {
  assert.equal(x.show("2026-10-21T19:35:26Z", { locale: "en-GB" }).text, "21 Oct 2026, 19:35 UTC");
  assert.equal(x.show("2026-10-21", { locale: "en-GB" }).text, "2026-10-21");
});

test("the map's tiles come only from the type's own tileset path (T-5189)", () => {
  const api = "https://api.dev.joinedcontext.com/api/endpoint/abc/";
  const at = x.tilesetUrl(api, "Bike");
  const of = (href, type = "application/vnd.mapbox-vector-tile") => x.templateOf({ links: [{ rel: "item", type, templated: true, href }] }, at);
  assert.equal(of(at + "/{tileMatrix}/{tileRow}/{tileCol}"), at + "/{z}/{y}/{x}");
  assert.equal(of("https://evil.example/t/{tileMatrix}/{tileRow}/{tileCol}"), null);
  assert.equal(of("http://antares:9471/ngsi-ld/ogc/collections/Bike/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"), null);
  assert.equal(of(at.replace("/Bike/", "/Car/") + "/{tileMatrix}/{tileRow}/{tileCol}"), null);
  assert.equal(of(at + "/{tileMatrix}/{tileRow}/{tileCol}?f=mvt"), null);
  assert.equal(of(at + "/{tileMatrix}/{tileCol}/{tileRow}"), null);
  assert.equal(of(at + "/{tileMatrix}/{tileRow}/{tileCol}", "application/json"), null);
  assert.equal(x.templateOf(null, at), null);
});

// DM-135 (T-5231): the row card a reader opens from the table or the map.
const T = { ...x.TEXT.sk, lang: "sk", locale: "sk-SK", timeZone: "Europe/Bratislava" };
const STOP = {
  type: "GtfsStop", typeLabel: "Železničná stanica", title: "name",
  fields: [
    { name: "name", label: "Názov" },
    { name: "dailyDepartures", label: "Odchody za deň", kind: "integer" },
    { name: "wheelchair", label: "Bezbariérová" },
    { name: "mode", label: "Druh", titles: { rail: "Vlak" } },
    { name: "stopCode", label: "Kód zastávky", hidden: true }
  ]
};

test("the card takes labels and order from the model and hides id, type, empty and system values", () => {
  const row = {
    id: "urn:ngsi-ld:GtfsStop:zilina-zariecie", type: "GtfsStop", "@context": "x", stopCode: "ZZ",
    mode: "rail", wheelchair: true, dailyDepartures: 1234, name: "Žilina-Záriečie", note: "", platformCount: null,
    location: { type: "Point", coordinates: [18.73109, 49.21933] }, dateModified: "2026-10-22T11:00:00Z",
    source: "https://data.slovensko.sk/datasety/abc?x=1", dataProvider: "ŽSR"
  };
  const card = x.cardOf(row, STOP, T);
  assert.equal(card.typeLabel, "Železničná stanica");
  assert.equal(card.title, "Žilina-Záriečie");
  assert.deepEqual(card.fields.map((f) => f.label), ["Odchody za deň", "Bezbariérová", "Druh"]);
  assert.equal(card.fields[0].text.replace(/\s/g, " "), "1 234");
  assert.equal(card.fields[1].text, "Áno");
  assert.equal(card.fields[2].text, "Vlak");
  assert.deepEqual(card.geo, row.location);
  assert.deepEqual(card.source, { provider: "ŽSR", href: "https://data.slovensko.sk/datasety/abc?x=1", host: "data.slovensko.sk" });
  assert.deepEqual(card.dev.map((d) => d.label), ["Identifikátor", "Typ", "Kód zastávky", "Date modified"]);
  assert.equal(JSON.stringify(card.fields).includes("urn:ngsi-ld"), false);
});

test("a type with no labels falls back to humanised keys and its type name made readable", () => {
  const card = x.cardOf({ id: "urn:ngsi-ld:GtfsStop:1", type: "GtfsStop", dailyDepartures: 3 }, null, T);
  assert.equal(card.typeLabel, "Gtfs stop");
  assert.equal(card.title, "urn:ngsi-ld:GtfsStop:1");
  assert.deepEqual(card.fields, [{ label: "Daily departures", text: "3", href: undefined }]);
  assert.equal(x.humanise("dailyDepartures"), "Daily departures");
  // Another type's card is never used for its labels.
  assert.equal(x.cardOf({ id: "a", type: "Event" }, STOP, T).typeLabel, "Event");
});

test("card values: local dates, units, an address by its parts, a web address by its host", () => {
  assert.equal(x.cardValue("2026-11-14T18:00:00Z", {}, T).text.replace(/\s/g, " "), "14. 11. 2026, 19:00");
  assert.equal(x.cardValue("2026-11-14", {}, T).text.replace(/\s/g, " "), "14. 11. 2026");
  assert.equal(x.cardValue(21.5, { unit: "°C" }, T).text, "21,5 °C");
  assert.equal(x.cardValue({ streetAddress: "Hlavná 1", addressLocality: "Žilina" }, {}, T).text, "Hlavná 1, Žilina");
  assert.deepEqual(x.cardValue("https://www.zilina.sk/podujatia/1", {}, T), { text: "zilina.sk", href: "https://www.zilina.sk/podujatia/1" });
  assert.equal(x.cardValue({ languageMap: { en: "Station", sk: "Stanica" } }, {}, T).text, "Stanica");
  assert.equal(x.cardValue(false, {}, T).text, "Nie");
  assert.equal(x.cardValue([], {}, T), null);
});

test("a javascript: or data: value renders as text, never a link", () => {
  for (const bad of ["javascript:alert(1)", "data:text/html,<b>x</b>", "JaVaScRiPt:alert(1)", "https://a b"]) {
    const said = x.cardValue(bad, {}, T);
    assert.equal(said.href, undefined, bad);
    assert.equal(said.text, bad);
    assert.equal(x.hostOf(bad), null);
  }
  const card = x.cardOf({ id: "a", type: "T", source: "javascript:alert(1)" }, null, T);
  assert.deepEqual(card.source, { text: "javascript:alert(1)" });
});

test("the removed default filter rides in the hash", () => {
  const view = { slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", inline: false, all: true, lang: null, theme: null, type: "Event", q: null, sort: null, dataset: null, view: null, geo: null, color: null, pop: null, cluster: null };
  assert.equal(x.hashOf(view), "#scsd2eehkx42n53z2zyd6vshfh7s7irf&all&type=Event");
  assert.deepEqual(x.parseHash(x.hashOf(view)), view);
});

// T-5233: the default NGSI-LD to map mapping and the reader's choice in the address.
const detailsOf = (attrs) => ({ typeName: "T", attributeDetails: Object.entries(attrs).map(([attributeName, kind]) => ({ attributeName, attributeTypes: [kind] })) });
const stationCard = { type: "Station", typeLabel: "Stanica", title: "name", fields: [
  { name: "name", label: "Názov" }, { name: "observationSpace", label: "Priestor pozorovania" }, { name: "location", label: "Poloha" },
  { name: "startDate", label: "Začiatok", kind: "date-time" }, { name: "address", label: "Adresa" }, { name: "url", label: "Web", kind: "uri" },
  { name: "status", label: "Stav", titles: { open: "Otvorené", closed: "Zatvorené" } }, { name: "secret", label: "Interné", hidden: true }
] };

test("the map draws location, else the first GeoProperty in model order, else nothing", () => {
  assert.deepEqual(x.geoProps(detailsOf({ observationSpace: "GeoProperty", name: "Property", location: "GeoProperty" }), stationCard), ["location", "observationSpace"]);
  assert.deepEqual(x.geoProps(detailsOf({ operationSpace: "GeoProperty", observationSpace: "GeoProperty" }), stationCard), ["observationSpace", "operationSpace"]);
  assert.deepEqual(x.geoProps(detailsOf({ name: "Property", refArea: "Relationship" }), stationCard), []);
  // A broker that lists only attribute names: location when the type has one.
  assert.deepEqual(x.geoProps({ typeName: "T", attributeNames: ["name", "location"] }, null), ["location"]);
  assert.deepEqual(x.geoProps({ attributeDetails: [{ attributeName: "../x", attributeTypes: ["GeoProperty"] }] }, null), []);
});

test("the default bubble holds the card's time, place and link; colour fields are enums and booleans", () => {
  const o = x.mapOptions(["location", "observationSpace"], stationCard);
  assert.deepEqual(o.defaults, { geo: "location", color: "type", pop: ["startDate", "address", "url"], cluster: true });
  assert.deepEqual(o.colors, ["none", "type", "status"]);
  assert.deepEqual(o.pops, ["startDate", "address", "url", "status"]);
  assert.equal(o.labels.observationSpace, "Priestor pozorovania");
  assert.deepEqual(x.mapOptions(["location"], null).defaults, { geo: "location", color: "type", pop: [], cluster: true });
});

test("a choice from the address is kept only when the model offers it; the rest falls back", () => {
  const o = x.mapOptions(["location", "observationSpace"], stationCard);
  const kept = x.mapChoice({ geo: "observationSpace", color: "status", pop: ["url"], cluster: false }, o);
  assert.deepEqual(kept, { choice: { geo: "observationSpace", color: "status", pop: ["url"], cluster: false }, unknown: [] });
  const odd = x.mapChoice({ geo: "secretArea", color: "secret", pop: ["url", "nothing"] }, o);
  assert.deepEqual(odd.choice, o.defaults);
  assert.deepEqual(odd.unknown, ["geo", "color", "pop"]);
  const view = { slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", inline: false, all: false, lang: null, theme: null, type: "Station", q: null, sort: null, dataset: null, view: null,
    geo: "observationSpace", color: "status", pop: [], cluster: false };
  assert.deepEqual(x.parseHash(x.hashOf(view)), view);
  const bad = x.parseHash("#scsd2eehkx42n53z2zyd6vshfh7s7irf&geo=a%22b&color=x;y&pop=a,,b&cluster=yes");
  assert.deepEqual([bad.geo, bad.color, bad.pop, bad.cluster], [null, null, null, null]);
});

// T-5239: the publisher's map defaults from the Portal (card.map) are where the map and its reset start.
test("the publisher's map defaults start the map when the model offers them; an invalid one keeps the built-in", () => {
  const card = { ...stationCard, map: { geometry: "observationSpace", colour: "status", popup: ["url", "status"], cluster: false } };
  const o = x.mapOptions(["location", "observationSpace"], card);
  assert.deepEqual(o.defaults, { geo: "observationSpace", color: "status", pop: ["url", "status"], cluster: false });
  // Nothing in the address: the reader's map is the publisher's; a reset (no raw choice) returns there.
  assert.deepEqual(x.mapChoice({}, o).choice, o.defaults);
  assert.deepEqual(x.mapChoice({ color: "none" }, o).choice.color, "none");
  const builtIn = x.mapOptions(["location", "observationSpace"], stationCard).defaults;
  for (const map of [
    { geometry: "secretArea" }, { colour: "secret" }, { colour: "name" }, { colour: "type" }, { popup: ["url", "nothing"] }, { popup: [] },
    { popup: Array.from({ length: 11 }, () => "url") }, { cluster: "no" }, { geometry: 3, colour: ["status"] },
  ]) {
    assert.deepEqual(x.mapOptions(["location", "observationSpace"], { ...stationCard, map }).defaults, builtIn, JSON.stringify(map));
  }
  // One member holds, another does not: each is checked alone.
  assert.deepEqual(x.mapOptions(["location"], { ...stationCard, map: { geometry: "observationSpace", colour: "status" } }).defaults,
    { ...builtIn, color: "status" });
});

test("a Polygon draws as an area, MultiPoints split into clusterable points, colours follow the field", () => {
  const t = { ...x.TEXT.sk, lang: "sk", locale: "sk-SK" };
  const area = { id: "a", geometry: { type: "Polygon", coordinates: [[[0, 0], [1, 0], [1, 1], [0, 0]]] }, properties: { type: "Station", status: "open" } };
  const many = { id: "m", geometry: { type: "MultiPoint", coordinates: [[1, 1], [2, 2]] }, properties: { type: "Station", status: "closed" } };
  const parts = x.splitPoints([area, many]);
  assert.deepEqual(parts.shapes.map((f) => f.id), ["a"]);
  assert.deepEqual(parts.points.map((f) => f.geometry.coordinates), [[1, 1], [2, 2]]);
  const c = x.colourClasses([area, many], "status", stationCard, t);
  assert.notEqual(c.colours[0], c.colours[1]);
  assert.deepEqual(c.legend.map((l) => l.label), ["Otvorené", "Zatvorené"]);
  assert.deepEqual(x.colourClasses([area, many], "type", stationCard, t).legend, []); // one type: no legend
  assert.equal(x.countLine(t, { inView: 412, total: 1680, none: 96 }), "Na mape 412 z 1 680 · 96 bez polohy");
  assert.match(x.countLine({ ...x.TEXT.en, locale: "en-GB" }, { inView: 5, total: 50000, none: 0, tiled: true }), /^On the map 5 of 50,000 · 0 without a location · drawn as tiles$/);
});

test("an organizer {name, url} reads as its name linked to its web site; without a safe url, as text (T-5134)", () => {
  const org = { name: "Obec Bíňovce", url: "https://www.binovce.sk" };
  assert.deepEqual(x.cardValue(org, {}, T), { text: "Obec Bíňovce", href: "https://www.binovce.sk" });
  assert.deepEqual(x.show(org, T), { text: "Obec Bíňovce", href: "https://www.binovce.sk" });
  assert.deepEqual(x.cardValue({ name: "Obec Bíňovce" }, {}, T), { text: "Obec Bíňovce" });
  for (const url of ["javascript:alert(1)", "binovce.sk", "https://a b"]) {
    assert.equal(x.cardValue({ name: "Obec Bíňovce", url }, {}, T).href, undefined, url);
    assert.equal(x.show({ name: "Obec Bíňovce", url }, T).href, undefined, url);
  }
  const card = x.cardOf({ id: "urn:ngsi-ld:Event:1", type: "Event", name: "Trh", organizer: org }, null, T);
  assert.deepEqual(card.fields, [{ label: "Organizer", text: "Obec Bíňovce", href: "https://www.binovce.sk" }]);
});
