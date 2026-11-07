/**
 * T-3901 (DM-82, DM-85): the Retention section of a space. The rules in plain units, what the
 * broker's sweeps deleted, and for an owner an editor that names what a shorter rule deletes
 * before it is proposed; a longer rule is proposed without the confirmation.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/SpaceRetention.tsx
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { SpaceRetention } from "../src/pages/spaces/SpaceRetention";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const BASE = "/api/v1/projects/helsinki/spaces/bikes";

const MANIFEST = {
  apiVersion: "joinedcontext.com/v1",
  kind: "ContextSpace",
  metadata: { name: "bikes", project: "helsinki" },
  spec: { retention: [{ entityType: "Vehicle", current: "PT5M", history: "PT8H" }] },
} as unknown as Manifest;

const DELETED = { counts: [{ entityType: "Vehicle", kind: "history", count: 42 }] };

function renderRetention(
  answer: (url: URL, request: Request) => Response | undefined | Promise<Response | undefined>,
) {
  return renderPage(
    <SpaceRetention project="helsinki" space="bikes" manifest={MANIFEST} types={["Stop", "Vehicle"]} />,
    { path: "/projects/helsinki/spaces/bikes", answer },
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the Retention section (DM-82, DM-85)", () => {
  it("shows each type's rule in plain units, the default where none, and what was deleted", async () => {
    const { container } = renderRetention((url) =>
      url.pathname === `${BASE}/retention/deleted` ? json(DELETED) : undefined,
    );
    const table = await screen.findByRole("table", { name: en.spaces.retention.caption });
    const vehicle = within(table).getByRole("row", { name: /Vehicle/ });
    expect(within(vehicle).getByText("5 minutes")).toBeInTheDocument();
    expect(within(vehicle).getByText("8 hours")).toBeInTheDocument();
    // T-4822: each cell names the layer that set it.
    expect(within(vehicle).getAllByText(en.spaces.retention.layer.type)).toHaveLength(2);
    expect(await within(vehicle).findByText("0 values, 42 history records")).toBeInTheDocument();
    const stop = within(table).getByRole("row", { name: /Stop/ });
    expect(within(stop).getByText(en.spaces.retention.forever)).toBeInTheDocument();
    expect(within(stop).getByText(en.spaces.retention.layer.none)).toBeInTheDocument();
    // T-5058: history is opt-in, so a type no rule names keeps none and writes no rows.
    expect(within(stop).getByText(en.spaces.retention.offShown)).toBeInTheDocument();
    expect(within(stop).getByText(en.spaces.retention.layer.platform)).toBeInTheDocument();
    expect(within(stop).getByText(en.spaces.retention.record.noRows)).toBeInTheDocument();
    // A type that keeps history writes a row per newer observation unless its rule says otherwise.
    expect(within(vehicle).getByText(en.spaces.retention.record.mode.observed)).toBeInTheDocument();
    // DM-97: the space's default row comes last and sets history only.
    const rows = within(table).getAllByRole("row");
    const any = rows[rows.length - 1];
    expect(within(any).getByText(en.spaces.retention.anyType)).toBeInTheDocument();
    // Its current values and its rows of history are set per type.
    expect(within(any).getAllByText(en.spaces.retention.perType)).toHaveLength(2);
    await expectNoAxeViolations(container);
  });

  it("shares one row among types under one rule, says heartbeats in words and since when it counts (T-6089)", async () => {
    const manifest = {
      ...MANIFEST,
      spec: {
        retention: [
          { entityType: "Vehicle", current: "PT5M", history: "PT8H" },
          { entityType: "KeyPerformanceIndicator", history: "P90D", record: "onChange", heartbeat: "P30D" },
        ],
      },
    } as unknown as Manifest;
    renderPage(
      <SpaceRetention project="helsinki" space="bikes" manifest={manifest} types={["Stop", "Station", "Vehicle", "Road"]} />,
      {
        path: "/projects/helsinki/spaces/bikes",
        answer: (url) =>
          url.pathname === `${BASE}/retention/deleted`
            ? json({ counts: [{ entityType: "Stop", kind: "entity", count: 2 }, { entityType: "Road", kind: "entity", count: 3 }] })
            : undefined,
      },
    );
    const table = await screen.findByRole("table", { name: en.spaces.retention.caption });
    // Vehicle, the KPI, one row for the three types without a rule, and the space default.
    expect(within(table).getAllByRole("row")).toHaveLength(5);
    const same = within(table).getByRole("row", { name: /3 types, same rule/ });
    expect(within(same).getByText("Road, Station, Stop")).toBeInTheDocument();
    expect(await within(same).findByText("5 values, 0 history records")).toBeInTheDocument();
    const kpi = within(table).getByRole("row", { name: /KeyPerformanceIndicator/ });
    expect(within(kpi).getByText("a row at least every 30 days")).toBeInTheDocument();
    expect(within(table).queryByText("P30D")).not.toBeInTheDocument();
    expect(screen.getByText(en.spaces.retention.deletedSince)).toBeInTheDocument();
    expect(within(table).getByRole("columnheader", { name: "Deleted since the broker started" })).toBeInTheDocument();
  });

  it("says the deletions are unknown when the broker does not answer", async () => {
    renderRetention((url) =>
      url.pathname === `${BASE}/retention/deleted`
        ? json({ type: "about:blank", title: "Unavailable", status: 503, detail: "no broker" }, 503)
        : undefined,
    );
    expect(await screen.findAllByText(en.spaces.retention.deletedUnknown)).toHaveLength(3);
  });

  it("a shorter rule names what it deletes before anything is proposed", async () => {
    const sent: { method: string; path: string; body: unknown }[] = [];
    renderRetention(async (url, request) => {
      if (url.pathname === `${BASE}/retention/deleted`) return json(DELETED);
      if (request.method === "POST" || request.method === "PUT") {
        sent.push({ method: request.method, path: url.pathname + url.search, body: await request.clone().json() });
        if (url.pathname === `${BASE}/retention/preview`) {
          return json({ counts: [{ entityType: "Vehicle", kind: "entity", count: 7 }] });
        }
        return json({ ...MANIFEST, status: {} });
      }
      return undefined;
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.spaces.retention.edit }));
    const amount = screen.getByRole("spinbutton", { name: "Current values of Vehicle: how many" });
    await user.clear(amount);
    await user.type(amount, "1");
    // A type kept forever has no amount to type: the unit says forever.
    expect(screen.queryByRole("spinbutton", { name: "Current values of Stop: how many" })).toBeNull();
    await user.click(screen.getByRole("button", { name: en.spaces.retention.save }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Vehicle: 7 current values")).toBeInTheDocument();
    expect(sent.map((one) => one.path)).toEqual([`${BASE}/retention/preview`]);
    expect(sent[0].body).toEqual({ retention: [{ entityType: "Vehicle", current: "PT1M", history: "PT8H" }] });
    await user.click(within(dialog).getByRole("button", { name: en.spaces.retention.confirm }));
    expect(await screen.findByText(en.spaces.retention.proposed)).toBeInTheDocument();
    // The proposal is checked first (PF-57), then sent.
    const puts = sent.filter((one) => one.method === "PUT");
    expect(puts.map((one) => one.path)).toEqual([`${BASE}?dryRun=All`, BASE]);
    const put = puts[1];
    expect((put?.body as { spec: { retention: unknown } }).spec.retention).toEqual([
      { entityType: "Vehicle", current: "PT1M", history: "PT8H" },
    ]);
  });

  it("a longer rule is proposed at once, and an amount out of range is refused in place", async () => {
    const sent: string[] = [];
    renderRetention((url, request) => {
      if (url.pathname === `${BASE}/retention/deleted`) return json(DELETED);
      if (request.method !== "GET") {
        sent.push(url.pathname + url.search);
        return json({ ...MANIFEST, status: {} });
      }
      return undefined;
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.spaces.retention.edit }));
    const amount = screen.getByRole("spinbutton", { name: "History of Vehicle: how many" });
    await user.clear(amount);
    await user.type(amount, "0");
    await user.click(screen.getByRole("button", { name: en.spaces.retention.save }));
    expect(await screen.findByText(en.spaces.retention.invalid)).toBeInTheDocument();
    expect(sent).toEqual([]);
    await user.clear(amount);
    await user.type(amount, "9");
    await user.click(screen.getByRole("button", { name: en.spaces.retention.save }));
    expect(await screen.findByText(en.spaces.retention.proposed)).toBeInTheDocument();
    expect(sent).toEqual([`${BASE}?dryRun=All`, BASE]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("offers history Off or a duration, never a new Forever, and confirms what Off deletes (T-4275)", async () => {
    const sent: { method: string; path: string; body: unknown }[] = [];
    renderRetention(async (url, request) => {
      if (url.pathname === `${BASE}/retention/deleted`) return json(DELETED);
      if (request.method === "POST" || request.method === "PUT") {
        sent.push({ method: request.method, path: url.pathname + url.search, body: await request.clone().json() });
        if (url.pathname === `${BASE}/retention/preview`) {
          return json({ counts: [{ entityType: "Stop", kind: "history", count: 1200 }] });
        }
        return json({ ...MANIFEST, status: {} });
      }
      return undefined;
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.spaces.retention.edit }));
    const options = (name: string) =>
      within(screen.getByRole("combobox", { name })).getAllByRole("option").map((option) => option.textContent);
    // A type with a history duration cannot be moved to forever; one kept forever today keeps
    // that as its present value only.
    expect(options("History of Vehicle: unit")).toEqual([
      en.spaces.retention.off,
      en.spaces.retention.unit.minutes,
      en.spaces.retention.unit.hours,
      en.spaces.retention.unit.days,
    ]);
    expect(options("History of Stop: unit")).toEqual([
      "No rule: no history (as now)",
      en.spaces.retention.off,
      en.spaces.retention.unit.minutes,
      en.spaces.retention.unit.hours,
      en.spaces.retention.unit.days,
    ]);
    expect(options("Current values of Stop: unit")).toContain(en.spaces.retention.forever);
    await user.selectOptions(screen.getByRole("combobox", { name: "History of Stop: unit" }), "off");
    expect(screen.queryByRole("spinbutton", { name: "History of Stop: how many" })).toBeNull();
    await user.click(screen.getByRole("button", { name: en.spaces.retention.save }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Stop: 1,200 history records")).toBeInTheDocument();
    expect(sent[0].body).toEqual({
      retention: [
        { entityType: "Stop", history: "off" },
        { entityType: "Vehicle", current: "PT5M", history: "PT8H" },
      ],
    });
  });

  it("shows a type whose history is off as Off", async () => {
    renderPage(
      <SpaceRetention
        project="helsinki"
        space="bikes"
        manifest={{ ...MANIFEST, spec: { retention: [{ entityType: "Stop", history: "off" }] } } as unknown as Manifest}
        types={["Stop"]}
      />,
      { path: "/projects/helsinki/spaces/bikes", answer: (url) => (url.pathname.endsWith("/deleted") ? json(DELETED) : undefined) },
    );
    const row = (await screen.findByRole("cell", { name: "Stop" })).closest("tr") as HTMLElement;
    expect(within(row).getByText(en.spaces.retention.offShown)).toBeInTheDocument();
  });

  it("the space's default covers the types without a history rule and is saved as * (DM-97)", async () => {
    const sent: { path: string; body: unknown }[] = [];
    const manifest = {
      ...MANIFEST,
      spec: { retention: [{ entityType: "Vehicle", current: "PT5M" }, { entityType: "*", history: "P90D" }] },
    } as unknown as Manifest;
    const { container } = renderPage(
      <SpaceRetention project="helsinki" space="bikes" manifest={manifest} types={["Stop", "Vehicle"]} />,
      {
        path: "/projects/helsinki/spaces/bikes",
        answer: async (url, request) => {
          if (url.pathname.endsWith("/deleted")) return json({ counts: [{ entityType: "*", kind: "history", count: 7 }] });
          if (request.method === "POST" || request.method === "PUT") {
            sent.push({ path: url.pathname, body: await request.clone().json() });
            if (url.pathname.endsWith("/preview")) return json({ counts: [{ entityType: "*", kind: "history", count: 9 }] });
            return json({ ...manifest, status: {} });
          }
          return undefined;
        },
      },
    );
    const table = await screen.findByRole("table", { name: en.spaces.retention.caption });
    const stop = within(table).getByRole("row", { name: /Stop/ });
    expect(within(stop).getByText("90 days")).toBeInTheDocument();
    expect(within(stop).getByText(en.spaces.retention.layer.space)).toBeInTheDocument();
    const any = within(table).getByRole("row", { name: new RegExp(en.spaces.retention.anyType) });
    expect(within(any).getByText("90 days")).toBeInTheDocument();
    expect(await within(any).findByText("0 values, 7 history records")).toBeInTheDocument();
    await expectNoAxeViolations(container);

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: en.spaces.retention.edit }));
    expect(screen.queryByRole("combobox", { name: `Current values of ${en.spaces.retention.anyType}: unit` })).toBeNull();
    const amount = screen.getByRole("spinbutton", { name: `History of ${en.spaces.retention.anyType}: how many` });
    await user.clear(amount);
    await user.type(amount, "30");
    await user.click(screen.getByRole("button", { name: en.spaces.retention.save }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(`${en.spaces.retention.anyType}: 9 history records`)).toBeInTheDocument();
    expect(sent[0].body).toEqual({
      retention: [
        { entityType: "Vehicle", current: "PT5M" },
        { entityType: "*", history: "P30D" },
      ],
    });
  });

  it("records a type's rows as chosen and keeps every other part of its rule (T-5058, DM-132)", async () => {
    const stored = {
      ...MANIFEST,
      spec: {
        retention: [
          {
            entityType: "Vehicle",
            history: "P30D",
            record: "onChange",
            heartbeat: "PT1H",
            attributes: { dateModified: { record: "never" } },
          },
          { entityType: "Stop", history: "P7D" },
        ],
      },
    } as unknown as Manifest;
    const sent: { method: string; path: string; body: unknown }[] = [];
    const { container } = renderPage(<SpaceRetention project="helsinki" space="bikes" manifest={stored} types={["Stop", "Vehicle"]} />, {
      path: "/projects/helsinki/spaces/bikes",
      answer: async (url, request) => {
        if (url.pathname === `${BASE}/retention/deleted`) return json(DELETED);
        if (request.method === "PUT") {
          sent.push({ method: request.method, path: url.pathname + url.search, body: await request.clone().json() });
          return json({ ...stored, status: {} });
        }
        return undefined;
      },
    });
    const table = await screen.findByRole("table", { name: en.spaces.retention.caption });
    const vehicle = within(table).getByRole("row", { name: /Vehicle/ });
    expect(within(vehicle).getByText(en.spaces.retention.record.mode.onChange)).toBeInTheDocument();
    expect(within(vehicle).getByText(en.pipelines.history.heartbeat.PT1H)).toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: en.spaces.retention.edit }));
    // Each select names its type and points at the hint with an example.
    const stop = screen.getByRole("combobox", { name: "Rows of history of Stop" });
    expect(stop).toHaveValue("observed");
    expect(stop).toHaveAccessibleDescription(en.spaces.retention.record.hint);
    expect(screen.getByText(en.spaces.retention.durationHint)).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Row at least every, for Vehicle" })).toHaveValue("PT1H");
    await user.selectOptions(stop, "onChange");
    await user.selectOptions(screen.getByRole("combobox", { name: "Row at least every, for Stop" }), "P1D");
    await user.selectOptions(screen.getByRole("combobox", { name: "Rows of history of Vehicle" }), "observed");
    expect(screen.queryByRole("combobox", { name: "Row at least every, for Vehicle" })).toBeNull();
    await expectNoAxeViolations(container);
    await user.click(screen.getByRole("button", { name: en.spaces.retention.save }));
    expect(await screen.findByText(en.spaces.retention.proposed)).toBeInTheDocument();
    expect(sent.map((one) => one.path)).toEqual([`${BASE}?dryRun=All`, BASE]);
    // Vehicle keeps its attribute recordings; onChange's heartbeat goes with observed.
    expect((sent[1]?.body as { spec: { retention: unknown } }).spec.retention).toEqual([
      { entityType: "Stop", history: "P7D", record: "onChange", heartbeat: "P1D" },
      { entityType: "Vehicle", history: "P30D", attributes: { dateModified: { record: "never" } }, record: "observed" },
    ]);
  });
});
