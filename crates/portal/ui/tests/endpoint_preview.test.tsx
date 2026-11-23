/**
 * AP-18, UI-33, R17, GW33 (T-2137): what the app generator says an endpoint gives you.
 *
 * The words come from the endpoint's own grant document, so the panel must never promise more
 * than the grant says: an attribute the policy holds back is named as held back, a grant with no
 * write is "read only", and a grant that names no type is not turned into a query — a read names
 * a type (GW33), and a query without one is refused by the gateway rather than answered.
 */
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EndpointPreview, accessWords } from "../src/pages/apps/EndpointPreview";
import { expectNoAxeViolations, json, renderPart } from "./page_contract";

const GRANT = {
  permissions: [
    { resource: { type: "AirQualityObserved" }, attributes: ["co2", "temperature"], actions: ["retrieveEntity"] },
    { resource: { type: "WeatherObserved" }, attributes: ["temperature"], actions: ["retrieveEntity", "updateAttrs"] },
  ],
  prohibitions: [{ attributes: ["personId"] }],
};

function show(answer: (url: URL) => Response | undefined) {
  return renderPart(<EndpointPreview slug="air-quality" />, { answer });
}

afterEach(() => vi.restoreAllMocks());

describe("the words a grant is read as", () => {
  it("names each type once, in order, and every attribute the grant lists", () => {
    expect(accessWords(GRANT)).toEqual({
      types: ["AirQualityObserved", "WeatherObserved"],
      attrs: ["co2", "temperature"],
      writes: expect.arrayContaining(["updateAttrs"]),
      denied: ["personId"],
    });
  });

  it("is `*` only when a permission itself is unlimited, never because a list is empty", () => {
    expect(accessWords({ permissions: [{ resource: { type: "A" }, attributes: "*" }] }).attrs).toBe("*");
    // No `attributes` key at all is the same promise, and an empty list is not.
    expect(accessWords({ permissions: [{ resource: { type: "A" } }] }).attrs).toBe("*");
    expect(accessWords({ permissions: [{ resource: { type: "A" }, attributes: [] }] }).attrs).toEqual([]);
  });

  it("says nothing at all for a document that never arrived", () => {
    expect(accessWords(undefined)).toEqual({ types: [], attrs: [], writes: [], denied: [] });
  });
});

describe("the preview of an endpoint", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  // UI-36: the preview names the types and attributes the endpoint's access document grants.
  it("says what the endpoint gives, the attributes it holds back included", async () => {
    show((url) => (url.pathname.endsWith("/access") ? json(GRANT) : json([])));
    expect(await screen.findByText(/Reads AirQualityObserved, WeatherObserved/)).toBeInTheDocument();
    expect(screen.getByText(/held back: personId/)).toBeInTheDocument();
  });

  // UI-36: the five-entity sample is read through the endpoint, by a type the grant names.
  it("asks for no sample while the grant names no type", async () => {
    const asked: string[] = [];
    show((url) => {
      asked.push(url.pathname);
      return url.pathname.endsWith("/access") ? json({ permissions: [] }) : json([]);
    });
    // "Reads this endpoint · all attributes · read only" is one sentence in one element.
    await screen.findByText(new RegExp(en.apps.generate.preview.reads));
    // A query with no type is 400 at the gateway (GW33): the panel says "no entities" instead.
    await waitFor(() => expect(screen.getByText(en.apps.generate.preview.samplesEmpty)).toBeInTheDocument());
    expect(asked.some((path) => path.includes("/entities"))).toBe(false);
  });

  // UI-36: the sample is a grid a person sorts by its id or by what the entities hold.
  it("draws the sample entities with their first values and sorts them by id and by values", async () => {
    const asked: string[] = [];
    const long = "x".repeat(60);
    show((url) => {
      if (url.pathname.endsWith("/access")) return json(GRANT);
      asked.push(`${url.pathname}${url.search}`);
      return json([
        { id: "urn:ngsi-ld:AirQualityObserved:hel:b-2", type: "AirQualityObserved", co2: 700, note: { a: 1 } },
        { id: "urn:ngsi-ld:AirQualityObserved:hel:a-1", type: "AirQualityObserved", co2: 410, note: long },
        { id: "urn:ngsi-ld:AirQualityObserved:hel:c-3", type: "AirQualityObserved", co2: 100 },
      ]);
    });
    await userEvent.click(await screen.findByText(en.apps.generate.preview.samplesCount.replace("{count}", "3")));
    const grid = await screen.findByRole("table", { name: en.apps.generate.preview.samples });
    expect(asked[0]).toContain("type=AirQualityObserved");
    expect(asked[0]).toContain("options=keyValues");
    const ids = () => within(grid).getAllByTitle(/^urn:ngsi-ld:/).map((cell) => cell.textContent);
    expect(within(grid).getByText(`co2=700, note={"a":1}`)).toBeInTheDocument();
    // A value past forty characters is cut with an ellipsis, so a cell stays one line.
    expect(within(grid).getByText(`co2=410, note=${"x".repeat(39)}…`)).toBeInTheDocument();

    const sortBy = (column: string, order: string) => {
      fireEvent.click(within(grid).getByRole("button", { name: `${en.entityGrid.columnMenu}: ${column}` }));
      fireEvent.click(screen.getByRole("menuitemradio", { name: order }));
    };
    sortBy(en.apps.generate.preview.columnId, en.entityGrid.sortAsc);
    expect(ids()).toEqual(["a-1", "b-2", "c-3"]);
    sortBy(en.apps.generate.preview.columnId, en.entityGrid.sortDesc);
    expect(ids()).toEqual(["c-3", "b-2", "a-1"]);
    sortBy(en.apps.generate.preview.columnValues, en.entityGrid.sortAsc);
    expect(ids()).toEqual(["c-3", "a-1", "b-2"]);
  });

  it("says the access is not stated rather than claiming none when the endpoint refuses", async () => {
    show((url) =>
      url.pathname.endsWith("/access")
        ? json({ type: "about:blank", title: "Forbidden", status: 403, detail: "no" }, 403)
        : json([]),
    );
    expect(await screen.findByText(en.apps.generate.preview.accessUnavailable)).toBeInTheDocument();
  });

  it("has no axe violation", async () => {
    const { container } = show((url) => (url.pathname.endsWith("/access") ? json(GRANT) : json([])));
    await screen.findByText(/Reads AirQualityObserved/);
    await expectNoAxeViolations(container);
  });
});
