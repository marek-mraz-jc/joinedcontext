// The page wiring of app.js (T-2597, T-3416): the served page read from its endpoint, the list,
// the map and the detail, the filters, and every way the read can fail, in a DOM.
import assert from "node:assert/strict";
import { afterEach, beforeEach, test, vi } from "vitest";
import EVENTS from "./events.json";
import SERVED from "../index.html?raw";

const PAGE = SERVED.replace(/<script[^>]*src=[^>]*><\/script>/, "");

/** The page as the static host serves it, `config` in its #jc-config, then app.js run on it. */
async function open(config, answer = async () => new Response(JSON.stringify(EVENTS), { status: 200 })) {
  document.documentElement.innerHTML = PAGE.replace(/^[\s\S]*<html[^>]*>/, "").replace(/<\/html>[\s\S]*$/, "");
  if (config !== undefined) {
    const script = document.createElement("script");
    script.id = "jc-config";
    script.type = "application/json";
    script.textContent = typeof config === "string" ? config : JSON.stringify(config);
    document.head.append(script);
  }
  const fetch = vi.fn(answer);
  vi.stubGlobal("fetch", fetch);
  vi.resetModules();
  await import("../app.js");
  return fetch;
}

const $ = (id) => document.getElementById(id);
const settled = () => vi.waitFor(() => assert.notEqual($("status").textContent, "Loading events…"));
const listed = () => [...$("events").querySelectorAll(".name")].map((name) => name.textContent);

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-09-22T09:00:00Z"));
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test("reads the events endpoint on its own origin and lists the upcoming events with a map", async () => {
  const fetch = await open({ slug: "ev" });
  await settled();
  assert.equal(fetch.mock.calls[0][0].startsWith("/api/endpoint/ev/ngsi-ld/v1/entities?"), true);
  assert.deepEqual(fetch.mock.calls[0][1], { headers: { Accept: "application/ld+json" }, credentials: "same-origin" });
  assert.equal($("from").value, "2026-09-22");
  assert.match($("status").textContent, /^\d+ of 5 events$/);
  assert.ok(listed().includes("Workshop for Families"));
  // Each part of a button is a word of its name: a screen reader hears the name, then the time.
  const family = [...$("events").querySelectorAll("button")].find((button) => button.textContent.startsWith("Workshop for Families"));
  assert.match(family.textContent, /^Workshop for Families \S/);
  assert.ok($("map").querySelectorAll("circle").length > 0);
});

test("opens an event from the list, focusing its detail, and from the map without moving the focus", async () => {
  await open({ slug: "ev" });
  await settled();
  const family = [...$("events").querySelectorAll("button")].find((button) => button.textContent.includes("Workshop for Families"));
  family.click();
  assert.equal($("detail-heading").textContent, "Workshop for Families");
  assert.equal(document.activeElement, $("detail"));
  assert.ok($("detail").textContent.includes("Where: Siltakatu 11, Helsinki"));
  assert.equal($("detail").querySelector("a").getAttribute("href"), "https://api.hel.fi/linkedevents/v1/");
  const pressed = [...$("events").querySelectorAll('button[aria-pressed="true"]')];
  assert.equal(pressed.length, 1);
  assert.equal($("map").querySelectorAll("circle.selected").length, 1);

  // A click draws the list again: each button is found anew.
  for (let index = 0; index < $("events").querySelectorAll("button").length; index += 1) $("events").querySelectorAll("button")[index].click();
  const dot = $("map").querySelector("circle:not(.selected)");
  const named = dot.querySelector("title").textContent;
  $("detail").blur();
  dot.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  assert.equal($("detail-heading").textContent, named);
  assert.notEqual(document.activeElement, $("detail"));
});

test("narrows by search and by the last day, and reads again from an earlier first day", async () => {
  const fetch = await open({ slug: "ev" });
  await settled();
  $("query").value = "cafe";
  $("query").dispatchEvent(new Event("input"));
  assert.deepEqual(listed(), ["Café concert"]);
  $("query").value = "";
  $("query").dispatchEvent(new Event("input"));
  $("to").value = "2026-10-31";
  $("to").dispatchEvent(new Event("change"));
  assert.ok(!listed().includes("Café concert"));
  $("from").value = "2026-01-01";
  $("from").dispatchEvent(new Event("change"));
  await vi.waitFor(() => assert.equal(fetch.mock.calls.length, 2));
  assert.equal(new URLSearchParams(fetch.mock.calls[1][0].split("?")[1]).get("q"), "endDate>=2026-01-01T00:00:00Z");
  // Enter in the search submits nothing: the page stays.
  const submit = new Event("submit", { cancelable: true });
  document.querySelector("form.filters").dispatchEvent(submit);
  assert.equal(submit.defaultPrevented, true);
});

test("says why there is nothing: no endpoint, a broken config, a refusal, a failed request, an odd body", async () => {
  await open(undefined);
  assert.match($("status").textContent, /no endpoint to read/);
  await open("{not json");
  assert.match($("status").textContent, /no endpoint to read/);
  await open({ slug: "ev" }, async () => new Response("{}", { status: 503 }));
  await settled();
  assert.equal($("status").textContent, "The events could not be read (HTTP 503).");
  await open({ slug: "ev" }, async () => {
    throw new TypeError("Failed to fetch");
  });
  await settled();
  assert.equal($("status").textContent, "The events could not be read: the network request failed.");
  await open({ slug: "ev" }, async () => new Response(JSON.stringify({ not: "a list" }), { status: 200 }));
  await settled();
  assert.equal($("status").textContent, "0 of 0 events");
});

test("shows an event without a place or dates, and one whose source is not https without a link", async () => {
  await open({ slug: "ev" });
  await settled();
  $("from").value = "2026-01-01";
  $("from").dispatchEvent(new Event("change"));
  await vi.waitFor(() => assert.ok(listed().includes("Open studio")));
  for (let index = 0; index < $("events").querySelectorAll("button").length; index += 1) {
    $("events").querySelectorAll("button")[index].click();
    const link = $("detail").querySelector("a");
    if (link) {
      assert.match(link.getAttribute("href"), /^https:/);
      // Following it leaves the page; jsdom does not navigate, so the click is only seen to reach it.
      link.addEventListener("click", (event) => event.preventDefault());
      link.click();
    }
  }
});
