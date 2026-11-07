import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import {
  apiOrigin,
  bySoonest,
  calendarUrl,
  choices,
  dayOf,
  eventOf,
  featuresOf,
  matches,
  monthDays,
  NO_FILTERS,
  onDay,
  parseWhen,
  rangeInvalid,
  safeLink,
  shiftMonth,
  webcalUrl,
} from "./events";
import type { EventItem } from "./events";
import { EVENTS, TOWNS } from "./fixtures/events";

const events = EVENTS.map((entity) => eventOf(toRichRow(entity, "sk"), "sk")).filter((event): event is EventItem => event !== null).sort(bySoonest);
const byId = (suffix: string) => events.find((event) => event.id.endsWith(`:${suffix}`)) as EventItem;

describe("events of the model", () => {
  it("reads every row with a name and a start, and leaves the one without a start out", () => {
    expect(events).toHaveLength(EVENTS.length - 1);
    expect(events.some((event) => event.name === "Podujatie bez dátumu")).toBe(false);
    expect(new Set(events.map((event) => event.town)).size).toBeGreaterThanOrEqual(10);
    expect(TOWNS).toHaveLength(14);
  });

  it("reads the town, venue, category, provider and point of an event", () => {
    const fair = byId("a1");
    expect(fair).toMatchObject({ town: "Banská Bystrica", venue: "Námestie SNP", category: "Jarmok", provider: "Mesto Banská Bystrica", coordinates: [19.1462, 48.7363], allDay: false });
    expect(fair.address).toContain("Banská Bystrica");
    expect(byId("o1").coordinates).toBeNull();
  });

  it("keeps the days in Slovakia: a date without time, several days, and an evening start", () => {
    expect(byId("b1")).toMatchObject({ startDay: "2026-10-29", endDay: "2026-10-31", allDay: true });
    expect(dayOf(new Date("2026-10-27T22:30:00Z"))).toBe("2026-10-27");
    expect(dayOf(new Date("2026-10-27T23:30:00Z"))).toBe("2026-10-28");
    expect(byId("d1").endDay).toBe("2026-11-08");
    expect(parseWhen("not a date")).toBeNull();
    expect(parseWhen("2026-13-45")).toBeNull();
    expect(parseWhen(null)).toBeNull();
  });

  it("keeps the start day when a source writes the end before the start", () => {
    const row = toRichRow({ id: "urn:ngsi-ld:Event:x:1", type: "Event", name: { type: "Property", value: "Chyba" }, startDate: { type: "Property", value: "2026-11-02" }, endDate: { type: "Property", value: "2026-11-01" } }, "sk");
    expect(eventOf(row, "sk")).toMatchObject({ startDay: "2026-11-02", endDay: "2026-11-02", town: null, address: null });
  });

  it("follows only http and https links", () => {
    expect(byId("m1").url).toBeNull();
    expect(byId("n1").url).toBeNull();
    expect(byId("a1").url).toBe("https://www.banskabystrica.sk/podujatia/jesenny-jarmok/");
    expect(safeLink("data:text/html,x")).toBeNull();
    expect(safeLink("not a url")).toBeNull();
    expect(safeLink("http://example.sk/a")).toBe("http://example.sk/a");
  });

  it("orders soonest first, then by name", () => {
    expect(events[0].name).toBe("Výstava fotografií Pohronie");
    const same = new Date("2026-11-01T10:00:00Z");
    const a = { ...events[0], start: same, name: "B" };
    const b = { ...events[0], start: same, name: "A" };
    expect([a, b].sort(bySoonest).map((event) => event.name)).toEqual(["A", "B"]);
  });
});

describe("filters", () => {
  it("offers each town and category once, in Slovak order", () => {
    const towns = choices(events, "town");
    expect(towns[0]).toBe("Banská Bystrica");
    expect(towns).toContain("Žiar nad Hronom");
    expect(towns.at(-1)).toBe("Žiar nad Hronom");
    expect(choices(events, "category")).toContain("Folklór");
  });

  it("filters by town and category", () => {
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, town: "Zvolen" })).map((event) => event.name)).toEqual(["Zvolenské divadelné dni", "Beseda o histórii zámku"]);
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, category: "Folklór" }))).toHaveLength(3);
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, town: "Zvolen", category: "Hudba" }))).toHaveLength(0);
  });

  it("keeps an event on any day of the range, a long exhibition included", () => {
    const november = events.filter((event) => matches(event, { ...NO_FILTERS, from: "2026-11-20", to: "2026-11-21" })).map((event) => event.name);
    expect(november).toEqual(["Výstava fotografií Pohronie", "Hontianska paráda – jesenná edícia"]);
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, from: "2026-10-31", to: "2026-10-31" })).map((event) => event.name)).toContain("Zvolenské divadelné dni");
  });

  it("searches name, venue, town and organiser without diacritics, every word", () => {
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, text: "zamok zvolen" })).map((event) => event.id)).toEqual([byId("b2").id]);
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, text: "  " }))).toHaveLength(events.length);
    expect(events.filter((event) => matches(event, { ...NO_FILTERS, text: "nič také" }))).toHaveLength(0);
  });

  it("says when From is after To", () => {
    expect(rangeInvalid({ ...NO_FILTERS, from: "2026-11-02", to: "2026-11-01" })).toBe(true);
    expect(rangeInvalid({ ...NO_FILTERS, from: "2026-11-01", to: "2026-11-01" })).toBe(false);
    expect(rangeInvalid({ ...NO_FILTERS, from: "2026-11-01" })).toBe(false);
  });
});

describe("calendar link", () => {
  const SLUG = "k2m4p6r8t0v2x4z6b8d0f2h4";

  it("names the platform's API host for an App on its own host, else the page's origin", () => {
    expect(apiOrigin({ protocol: "https:", host: "bbsk-podujatia.apps.dev.joinedcontext.com" })).toBe("https://api.dev.joinedcontext.com");
    expect(apiOrigin({ protocol: "http:", host: "portal.test" })).toBe("http://portal.test");
    expect(apiOrigin({ protocol: "http:", host: "localhost:5173" })).toBe("http://localhost:5173");
  });

  it("links the endpoint's iCalendar feed, the whole region or its days", () => {
    const origin = "https://api.dev.joinedcontext.com";
    expect(calendarUrl(origin, SLUG)).toBe(`${origin}/api/endpoint/${SLUG}/ical/v1/events.ics`);
    expect(calendarUrl(origin, SLUG, { from: "2026-11-01", to: "2026-11-30" })).toBe(`${origin}/api/endpoint/${SLUG}/ical/v1/events.ics?from=2026-11-01&to=2026-11-30`);
    expect(calendarUrl(origin, SLUG, { from: "", to: "2026-11-30" })).toBe(`${origin}/api/endpoint/${SLUG}/ical/v1/events.ics?to=2026-11-30`);
  });

  it("never carries anything but a date into the link", () => {
    const url = calendarUrl("https://api.x.sk", "a/b", { from: "2026-11-01&q=x", to: "zajtra" });
    expect(url).toBe("https://api.x.sk/api/endpoint/a%2Fb/ical/v1/events.ics");
  });

  it("opens a calendar application with webcal", () => {
    expect(webcalUrl("https://api.x.sk/a.ics?from=2026-11-01")).toBe("webcal://api.x.sk/a.ics?from=2026-11-01");
    expect(webcalUrl("http://portal.test/a.ics")).toBe("webcal://portal.test/a.ics");
  });
});

describe("month", () => {
  it("draws whole weeks from Monday", () => {
    const november = monthDays("2026-11");
    expect(november[0]).toBe("2026-10-26");
    expect(november).toHaveLength(42);
    expect(november.at(-1)).toBe("2026-12-06");
    expect(monthDays("2027-02")).toHaveLength(28);
    expect(monthDays("2027-02")[0]).toBe("2027-02-01");
  });

  it("moves across a year", () => {
    expect(shiftMonth("2026-12", 1)).toBe("2027-01");
    expect(shiftMonth("2026-01", -1)).toBe("2025-12");
  });

  it("puts a several-day event on each of its days", () => {
    expect(onDay(events, "2026-10-30").map((event) => event.name)).toEqual(["Výstava fotografií Pohronie", "Zvolenské divadelné dni"]);
    expect(onDay(events, "2026-12-31")).toHaveLength(0);
  });
});

describe("map", () => {
  it("draws the placed events and marks the picked one", () => {
    const drawn = featuresOf(events, byId("a1").id);
    expect(drawn.features).toHaveLength(events.length - 1);
    expect(drawn.features.find((feature) => feature.id === byId("a1").id)?.properties.picked).toBe(true);
  });
});
