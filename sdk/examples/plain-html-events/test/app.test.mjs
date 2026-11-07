// @vitest-environment node
// The pure functions of the Helsinki events page (T-2597), on five events in the shape the
// Helsinki events endpoint answers on dev: normalized NGSI-LD, LanguageProperty names.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "vitest";
import { dayOf, eventsUrl, filterEvents, pickEndpoint, project, text, toView, when } from "../app.js";

// Five events in the shape the endpoint answers, shared with the browser flow
// (ui/e2e/app_helsinki_events.spec.ts). One has ended, one has no dates or place, and one has a
// source that is not an https link, which must never be rendered as one.
const EVENTS = JSON.parse(readFileSync(new URL("./events.json", import.meta.url), "utf8"));

const views = EVENTS.map((entity) => toView(entity, ["en"]));
const names = (list) => list.map((event) => event.name);

// AP-04: the page reads the endpoint serving Event, whatever the host's primary is.
test("the endpoint is the events one, else one serving Event, else the primary", () => {
  const events = { name: "helsinki-events", slug: "ev", types: [] };
  const alerts = { name: "helsinki-alerts", slug: "al", types: ["Alert"] };
  assert.equal(pickEndpoint({ slug: "al", endpoints: [alerts, events] }), "ev");
  assert.equal(pickEndpoint({ slug: "al", endpoints: [alerts, { name: "app-x", slug: "ax", types: ["Event"] }] }), "ax");
  assert.equal(pickEndpoint({ slug: "al", endpoints: [alerts] }), "al");
  assert.equal(pickEndpoint({}), null);
  assert.equal(pickEndpoint(null), null);
});

// AP-07, AP-11: one same-origin path on the app's endpoint, the slug escaped, the date unquoted.
test("the query asks for events not ended before the start day", () => {
  const url = eventsUrl("a b", new Date("2026-09-22T00:00:00.000Z"));
  assert.ok(url.startsWith("/api/endpoint/a%20b/ngsi-ld/v1/entities?"), url);
  const query = new URLSearchParams(url.split("?")[1]);
  assert.equal(query.get("type"), "Event");
  assert.equal(query.get("q"), "endDate>=2026-09-22T00:00:00Z");
  assert.equal(new URLSearchParams(eventsUrl("s", null).split("?")[1]).get("q"), null);
});

// AP-14: what the served page shows of each entity, whatever the endpoint omits.
test("an entity becomes what the page shows, in the reader's language", () => {
  const [family] = views;
  assert.equal(family.name, "Workshop for Families");
  assert.equal(toView(EVENTS[0], ["fi"]).name, "Perhepaja");
  assert.equal(toView(EVENTS[0], ["de"]).name, "Workshop for Families", "an unknown language falls back to en");
  assert.equal(family.address, "Siltakatu 11, Helsinki");
  assert.deepEqual(family.point, { lon: 24.657864, lat: 60.203636 });
  assert.equal(family.start.toISOString(), "2026-10-20T14:00:00.000Z");
  assert.equal(family.source, "https://api.hel.fi/linkedevents/v1/");

  assert.equal(views[3].source, "", "a non-https source is dropped");
  assert.equal(views[4].start, null);
  assert.equal(views[4].point, null);
  assert.equal(toView({ type: "Event" }), null, "no id, not an entity");
  assert.equal(toView(null), null);
  // The simplified form reads the same.
  assert.equal(toView({ id: "x", name: "Plain", startDate: "not a date" }).start, null);
  assert.equal(text({ languageMap: { sv: "Bara svenska" } }, ["en"]), "Bara svenska");
});

// AP-14: the upcoming events a reader asked for.
test("the date filter keeps what overlaps the days asked for, soonest first", () => {
  const from = dayOf("2026-09-22");
  assert.deepEqual(names(filterEvents(views, { from })), [
    "Business plan calculations video", // running since 2024, ends in December
    "Workshop for Families",
    "Café concert",
    "Open studio", // no dates: never filtered out by a date
  ]);
  assert.deepEqual(names(filterEvents(views, { from, to: dayOf("2026-10-31", true) })), [
    "Business plan calculations video",
    "Workshop for Families",
    "Open studio",
  ]);
  assert.equal(filterEvents(views, {}).length, 5, "no filter, everything");
  assert.equal(filterEvents([], { from }).length, 0);
});

// AP-14: search over the events the page holds.
test("search matches every word in name, place or description, ignoring case and accents", () => {
  assert.deepEqual(names(filterEvents(views, { query: "cafe" })), ["Café concert"]);
  assert.deepEqual(names(filterEvents(views, { query: "  SILTAKATU  families " })), ["Workshop for Families"]);
  assert.deepEqual(names(filterEvents(views, { query: "register" })), ["Workshop for Families"]);
  assert.deepEqual(filterEvents(views, { query: "concert siltakatu" }), [], "every word, not any");
  assert.equal(filterEvents(views, { query: "   " }).length, 5, "blank search is no search");
});

// AP-14: dates in the reader's input and in Helsinki time.
test("the date input and the date line", () => {
  assert.equal(dayOf(""), null);
  assert.equal(dayOf("2026-13-45"), null);
  assert.equal(dayOf("22.9.2026"), null);
  assert.ok(dayOf("2026-09-22", true) > dayOf("2026-09-22"));
  assert.match(when(views[0]), /20 Oct 2026, 17:00 – 20 Oct 2026, 19:00/, "Helsinki time");
  assert.equal(when(views[4]), "date not given");
});

// AP-14: the map is drawn from the entities, with no tile server (AP-11: no other host).
test("the map places the events north up inside the box, and draws nothing for none", () => {
  const points = project(views, 320, 240);
  assert.equal(points.length, 4, "the undated event has no location");
  for (const { x, y } of points) {
    assert.ok(x >= 0 && x <= 320 && y >= 0 && y <= 240, `${x},${y}`);
  }
  const byId = Object.fromEntries(points.map((p) => [p.id.split(":").pop(), p]));
  assert.ok(byId.past.y < byId["elo-video"].y, "the northern event is drawn higher");
  assert.ok(byId["espoo_le-agn5nkl76q"].x < byId["elo-video"].x, "the western event is drawn left");
  assert.deepEqual(project([], 320, 240), []);
  const one = project([views[0]], 320, 240);
  assert.equal(one.length, 1);
  assert.ok(Number.isFinite(one[0].x) && Number.isFinite(one[0].y), "a single point does not divide by zero");
});

// The edges of the shapes the endpoint may send, each read as nothing rather than as a wrong word.
test("a value with nothing to read is an empty word, a nameless event is its id, a half-dated one says its one end", () => {
  assert.equal(text({ languageMap: { fi: 5 } }, ["en"]), "");
  assert.equal(text({ value: null }), "");
  assert.equal(text({ value: 12 }), "12");
  assert.equal(toView({ id: "urn:ngsi-ld:Event:x" }).name, "urn:ngsi-ld:Event:x");
  const start = toView({ id: "a", startDate: "2026-10-01T10:00:00Z" });
  const end = toView({ id: "b", endDate: "2026-10-02T10:00:00Z" });
  assert.match(when(start), /^from /);
  assert.match(when(end), /^until /);
  assert.equal(dayOf(undefined), null);
  // Two events without a start keep their order after the dated ones.
  const undated = [toView({ id: "c", name: "C" }), toView({ id: "d", name: "D" })];
  assert.deepEqual(names(filterEvents([...undated, start], {})), ["a", "C", "D"]);
});
