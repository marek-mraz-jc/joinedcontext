/**
 * T-3262: before a pipeline is proposed, its mapped records against the entities the space holds
 * now: created, updated (with what changes) or no change, read with the person's session through
 * the space surface; nothing is written.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SpaceChanges } from "../src/pages/pipelines/SpaceChanges";
import { changesOf, countsOf } from "../src/pages/pipelines/spaceChanges";

const c = en.pipelines.workbench.changes;
const urn = (n: string) => `urn:ngsi-ld:AirQualityObserved:hel.fi:air:${n}`;
const RECORDS = [
  { id: urn("new"), type: "AirQualityObserved", pm10: { type: "Property", value: 4 } },
  { id: urn("same"), type: "AirQualityObserved", pm10: { type: "Property", value: 5 } },
  { id: urn("moved"), type: "AirQualityObserved", pm10: { type: "Property", value: 9 }, name: { type: "Property", value: "Kallio" } },
  { type: "AirQualityObserved" },
];
const NOW = [
  { id: urn("same"), type: "AirQualityObserved", pm10: { type: "Property", value: 5, modifiedAt: "2026-10-07T01:00:00Z" }, other: { type: "Property", value: 1 } },
  { id: urn("moved"), type: "AirQualityObserved", pm10: { type: "Property", value: 7, unitCode: "GQ" } },
];

describe("what a pipeline would change (PL-70)", () => {
  it("classifies each record by id, comparing only what the record writes", () => {
    const changes = changesOf(RECORDS, new Map(NOW.map((entity) => [entity.id, entity])));
    expect(changes.map((change) => [change.id, change.outcome])).toEqual([
      [urn("new"), "create"],
      [urn("same"), "unchanged"],
      [urn("moved"), "update"],
    ]);
    expect(changes[2].changes).toEqual([
      { path: "pm10.value", kind: "changed", before: 7, after: 9 },
      { path: "pm10.unitCode", kind: "removed", before: "GQ" },
      { path: "name", kind: "added", after: { type: "Property", value: "Kallio" } },
    ]);
    expect(countsOf(changes)).toEqual({ create: 1, update: 1, unchanged: 1 });
  });
});

describe("the comparison in the workbench (PL-70)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads the space with GET only and shows the counts and each entity's change", async () => {
    const fetchMock = vi.fn(() =>
      Promise.resolve(new Response(JSON.stringify(NOW), { status: 200, headers: { "Content-Type": "application/json" } })),
    );
    vi.stubGlobal("fetch", fetchMock);
    render(
      <I18nextProvider i18n={i18n}>
        <SpaceChanges space="air" records={RECORDS} />
      </I18nextProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: c.compare }));
    expect(await screen.findByRole("status")).toHaveTextContent("1 to create, 1 to update, 1 unchanged.");
    const calls = fetchMock.mock.calls as unknown as [string, RequestInit][];
    expect(calls).toHaveLength(1);
    expect(calls[0][1].method).toBe("GET");
    expect(calls[0][0]).toBe(
      `/cs/air/ngsi-ld/v1/entities?id=${[urn("new"), urn("same"), urn("moved")].map(encodeURIComponent).join(",")}&limit=100`,
    );
    const row = within(screen.getByRole("table")).getByRole("row", { name: new RegExp(urn("moved")) });
    expect(row).toHaveTextContent(c.outcomes.update);
    expect(row).toHaveTextContent("changed pm10.value: 7 → 9");
  });

  it("says why the space could not be read", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify({ title: "Forbidden", detail: "no read on air" }), { status: 403, headers: { "Content-Type": "application/problem+json" } }))),
    );
    render(
      <I18nextProvider i18n={i18n}>
        <SpaceChanges space="air" records={RECORDS} />
      </I18nextProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: c.compare }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The space could not be read: no read on air");
  });

  it("offers no comparison for records without ids", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <SpaceChanges space="air" records={[{ type: "AirQualityObserved" }]} />
      </I18nextProvider>,
    );
    expect(screen.getByRole("button", { name: c.compare })).toHaveAttribute("aria-disabled", "true");
  });
});
