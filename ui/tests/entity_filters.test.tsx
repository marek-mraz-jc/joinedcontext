/** T-0517: filters generated from the DataModel, shared by the explorer and the pipeline studio (UI-33, DM-20, PL-42). */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/entities/EntityFilters.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import type { Manifest } from "../src/api/manifest";
import { viewerWriteFindings } from "./pageChecks";
import { filterSlotsOf, filtersFromQ, opsFor, qFromFilters, qProblem, typeGroups } from "../src/components/entities/filters";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const BRANDING = {
  name: "Portal",
  colors: { primary: "#1d4ed8", background: "#ffffff", text: "#0f172a" },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const AIR_MODEL = [
  "id: https://banskabystrica.sk/models/air",
  "name: bb-air-quality",
  "classes:",
  "  AirQualityObserved:",
  "    slots: [id, pm10, active, level, refDistrict, dateObserved]",
  "slots:",
  "  id: {}",
  "  pm10: { range: float, minimum_value: 0 }",
  "  active: { range: boolean }",
  "  level: { range: AirLevel }",
  "  refDistrict:",
  "    range: uri",
  "    annotations: { ngsi_ld_kind: Relationship }",
  "  dateObserved: { range: datetime }",
  "enums:",
  "  AirLevel:",
  "    permissible_values: { good: {}, bad: {} }",
  "",
].join("\n");

const MODEL: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bb-air-quality", namespace: "banskabystrica" },
  spec: { linkml: AIR_MODEL, version: "1.0.0", classes: ["AirQualityObserved"] },
};
/** How a committed model reaches the UI: `spec.linkml` is the path of the file in the repository. */
const COMMITTED: Manifest = {
  ...MODEL,
  spec: { linkml: "models/bb-air-quality.yaml", version: "1.0.0", classes: ["AirQualityObserved"] },
};
const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "ovzdusie", namespace: "banskabystrica", title: { en: "Air quality" } },
    spec: { dataModelRef: "bb-air-quality" },
  },
]);
const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "banskabystrica" },
    spec: { contextSpaceRef: "ovzdusie", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);
const ROWS = [
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:radvan-01", type: "AirQualityObserved", pm10: 12 },
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:sasova-01", type: "AirQualityObserved", pm10: 30 },
];
const FULL = {
  id: ROWS[1].id,
  type: "AirQualityObserved",
  pm10: { type: "Property", value: 30, observedAt: "2026-09-07T10:00:00Z" },
};

describe("filters from the model", () => {
  const slots = filterSlotsOf(MODEL, "AirQualityObserved");

  it("types every slot of the class by its range, enum and kind", () => {
    expect(slots.map((s) => s.name)).toEqual(["pm10", "active", "level", "refDistrict", "dateObserved"]);
    expect(slots[0]).toMatchObject({ range: "float", minimum: 0 });
    expect(slots[2].values).toEqual(["good", "bad"]);
    expect(slots[3].kind).toBe("Relationship");
    expect(filterSlotsOf(MODEL, "Nope")).toEqual([]);
    expect(filterSlotsOf(undefined, "AirQualityObserved")).toEqual([]);
  });

  it("takes the model as text, and a manifest that only points at the file gives nothing (T-0797)", () => {
    expect(filterSlotsOf(AIR_MODEL, "AirQualityObserved")).toEqual(slots);
    expect(filterSlotsOf(COMMITTED, "AirQualityObserved")).toEqual([]);
    expect(filterSlotsOf("", "AirQualityObserved")).toEqual([]);
  });

  it("offers the operators the range allows", () => {
    expect(opsFor(slots[0])).toEqual(["==", "!=", ">", ">=", "<", "<="]);
    expect(opsFor(slots[1])).toEqual(["==", "!="]);
    expect(opsFor(slots[2])).toEqual(["==", "!="]);
    expect(opsFor(slots[3])).toEqual(["==", "!="]);
    expect(opsFor(slots[4])).toEqual(["==", "!=", ">", ">=", "<", "<="]);
    expect(opsFor({ name: "name", kind: "Property", range: "string" })).toEqual(["==", "!=", "~=", "!~="]);
    expect(opsFor(undefined)).toHaveLength(8);
  });

  it("composes q with bare numbers, booleans and dates and quoted strings, and parses it back", () => {
    const rows = [
      { attribute: "pm10", op: ">" as const, value: "10" },
      { attribute: "active", op: "==" as const, value: "true" },
      { attribute: "level", op: "!=" as const, value: "bad" },
      { attribute: "refDistrict", op: "==" as const, value: "urn:ngsi-ld:District:bb:d:1" },
      { attribute: "dateObserved", op: ">=" as const, value: "2026-09-01T00:00:00Z" },
      { attribute: "pm10", op: "<" as const, value: "" },
    ];
    const q = qFromFilters(rows, slots);
    expect(q).toBe(
      'pm10>10;active==true;level!="bad";refDistrict=="urn:ngsi-ld:District:bb:d:1";dateObserved>=2026-09-01T00:00:00Z',
    );
    expect(filtersFromQ(q)).toEqual(rows.slice(0, 5));
    // A quote inside a string survives the round trip.
    expect(filtersFromQ(qFromFilters([{ attribute: "name", op: "~=", value: 'a"b' }]))).toEqual([
      { attribute: "name", op: "~=", value: 'a"b' },
    ]);
    // Without a slot the value's shape decides: numbers bare, words quoted.
    expect(qFromFilters([{ attribute: "x", op: "==", value: "3" }, { attribute: "y", op: "==", value: "z" }])).toBe(
      'x==3;y=="z"',
    );
  });

  /// T-4264, T-4160 #22: what the broker would refuse with an English parser error is caught in
  /// the field, with the fix; what it takes passes.
  it("finds the q the broker would refuse, and says how to fix text without quotes", () => {
    for (const fine of [
      undefined,
      "",
      'name=="Nemocnica"',
      "pm10>20;active==true",
      "pm10>10|pm10<1",
      "dateObserved>=2026-10-01T00:00:00Z",
      "pm10==1..5",
      'level=="good","bad"',
      "address[streetAddress]==\"Hlavná 1\"",
      "refDistrict",
      "!refDistrict",
      "(pm10>1|pm10<0);active==true",
      'name~="^Nem.*"',
    ]) {
      expect(qProblem(fine), String(fine)).toBeUndefined();
    }
    expect(qProblem("name==qzxwv-nothing-is-called-this")).toEqual({
      kind: "unquoted",
      fix: 'name=="qzxwv-nothing-is-called-this"',
    });
    expect(qProblem('pm10>1;name==a"b')).toEqual({ kind: "unquoted", fix: 'name=="a\\"b"' });
    for (const broken of ['name == "x"', "pm10>", "==5", "pm10>1;;", 'name=="unclosed']) {
      expect(qProblem(broken), broken).toBeDefined();
    }
  });

  it("leaves q the rows cannot show as text", () => {
    expect(filtersFromQ("")).toEqual([]);
    expect(filtersFromQ(undefined)).toEqual([]);
    expect(filtersFromQ("pm10>10|pm25>5")).toBeUndefined();
    expect(filtersFromQ("(pm10>10)")).toBeUndefined();
    expect(filtersFromQ("pm10")).toBeUndefined();
    expect(filtersFromQ("pm10.value>10")).toEqual([{ attribute: "pm10.value", op: ">", value: "10" }]);
  });
});

/** The URL of one call: the page sends a `Request`, the SDK grid's transport a path (T-1432). */
function urlOf(input: unknown): URL {
  const raw = typeof input === "string" ? input : (input as Request).url;
  return new URL(raw, window.location.origin);
}

function renderExplore(model: Manifest = MODEL) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = urlOf(input);
    const json = (body: unknown, headers: Record<string, string> = {}) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json", ...headers },
        }),
      );
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/branding")) return json(BRANDING);
    if (url.pathname.includes("/ngsi-ld/v1/entities/")) return json(FULL);
    if (url.pathname.includes("/ngsi-ld/v1/entities")) {
      const q = url.searchParams.get("q");
      return json(q ? ROWS.filter((row) => row.pm10 > 20) : ROWS, { "NGSILD-Results-Count": q ? "1" : "2" });
    }
    if (url.pathname.endsWith("/spaces")) return json(SPACES);
    if (url.pathname.endsWith("/endpoints")) return json(ENDPOINTS);
    if (url.pathname.endsWith("/datamodels")) return json(list([model]));
    // The source route DM-56 serves for a model the repository holds as a file.
    if (url.pathname.endsWith("/datamodels/bb-air-quality/source")) {
      return Promise.resolve(new Response(AIR_MODEL, { status: 200, headers: { "Content-Type": "text/yaml" } }));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

const gatewayUrls = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls
    .map((call) => urlOf(call[0]))
    .filter((url) => url.pathname.includes("/ngsi-ld/v1/entities"));

describe("data explorer (UI-33)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/explore");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("reads a page through the space's endpoint with a filter generated from the model, then one entity in full", async () => {
    const fetchMock = renderExplore();
    await screen.findByRole("heading", { name: en.explore.title });
    // The spaces arrive after the select renders: wait for the option, not the label (CI load race).
    const space = await screen.findByLabelText(en.explore.space);
    await waitFor(() => expect(space.querySelector('option[value="ovzdusie"]')).not.toBeNull());
    await userEvent.selectOptions(space, "ovzdusie");
    expect(screen.getByLabelText(en.explore.endpoint)).toHaveValue("public-air");
    await userEvent.selectOptions(screen.getByLabelText(en.entities.type), "AirQualityObserved");

    // The whole type first: two rows and the broker's count.
    // The row shows its URN's end and is opened by it, the whole id its tooltip (T-4265, T-5619).
    expect(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: sasova-01` })).toHaveAttribute("title", ROWS[1].id);
    // The endpoint's own count, as the grid's footer says it (R22).
    expect(screen.getByText(`2 ${en.entityGrid.of} 2 ${en.entityGrid.loaded}`)).toBeInTheDocument();

    // A row from the model: pm10 is a float, so the value is a number input and > is offered.
    await userEvent.click(screen.getByRole("button", { name: en.entities.addFilter }));
    const value = screen.getByLabelText(en.entities.value);
    expect(value).toHaveAttribute("type", "number");
    await userEvent.selectOptions(screen.getByLabelText(en.entities.operator), ">");
    await userEvent.type(value, "20");
    expect(screen.getByLabelText(en.entities.q)).toHaveValue("pm10>20");
    await waitFor(() => expect(screen.queryByRole("button", { name: `${en.entityGrid.openRow}: radvan-01` })).not.toBeInTheDocument());
    // The rows and the count are two reads of one question (T-6541).
    const asked = gatewayUrls(fetchMock).filter((url) => url.searchParams.get("q") === "pm10>20");
    expect(asked.every((url) => url.pathname.includes("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/"))).toBe(true);
    expect(asked.some((url) => url.searchParams.get("count") === "true" && url.searchParams.get("limit") === "0")).toBe(true);
    expect(asked.some((url) => url.searchParams.get("count") === null && url.searchParams.get("limit") !== "0")).toBe(true);
    expect(screen.getByText(`1 ${en.entityGrid.of} 1 ${en.entityGrid.loaded}`)).toBeInTheDocument();

    // A q typed by hand outside the simple grammar shows as text, no rows.
    await userEvent.clear(screen.getByLabelText(en.entities.q));
    await userEvent.type(screen.getByLabelText(en.entities.q), "pm10>10|pm10<1");
    expect(screen.getByText(en.entities.advanced)).toBeInTheDocument();

    // Text without its quotes is caught on the field with the fix, and never sent (T-4160 #22).
    const sent = gatewayUrls(fetchMock).length;
    await userEvent.clear(screen.getByLabelText(en.entities.q));
    await userEvent.type(screen.getByLabelText(en.entities.q), "name==qzxwv");
    expect(screen.getByText(en.entities.qProblem.unquoted.replace("{fix}", 'name=="qzxwv"'))).toBeInTheDocument();
    expect(screen.queryByText(en.entities.advanced)).toBeNull();
    expect(gatewayUrls(fetchMock).slice(sent).some((url) => url.searchParams.get("q")?.includes("qzxwv"))).toBe(false);
    await userEvent.clear(screen.getByLabelText(en.entities.q));

    // One entity in full, in the grid's own detail; its JSON-LD read by id once asked for (T-5619).
    await userEvent.click(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: sasova-01` }));
    const detail = await screen.findByRole("dialog");
    expect(detail.textContent).not.toContain("{\n");
    await userEvent.click(within(detail).getByText(en.explore.technical));
    await within(detail).findByRole("group", { name: en.codeBlock.entity });
    // The entity was read by its id through the same endpoint; the grid's own list read may be the
    // last call of all, so what matters is that this one happened at all.
    expect(
      gatewayUrls(fetchMock).some((url) => url.pathname.includes(encodeURIComponent(ROWS[1].id))),
    ).toBe(true);
  });

  it("generates the rows of a model the repository holds as a file (T-0797)", async () => {
    const fetchMock = renderExplore(COMMITTED);
    await screen.findByRole("heading", { name: en.explore.title });
    const space = await screen.findByLabelText(en.explore.space);
    await waitFor(() => expect(space.querySelector('option[value="ovzdusie"]')).not.toBeNull());
    await userEvent.selectOptions(space, "ovzdusie");
    await userEvent.selectOptions(await screen.findByLabelText(en.entities.type), "AirQualityObserved");

    // The source route carried the LinkML the manifest only names, so the rows are typed.
    // Add filter only edits the query in hand: the viewer-write walker check passes it (T-6139).
    expect(viewerWriteFindings(false)).not.toContain(`viewer-write: "${en.entities.addFilter}" can be pressed`);
    await userEvent.click(screen.getByRole("button", { name: en.entities.addFilter }));
    await waitFor(() => expect(screen.getByLabelText(en.entities.value)).toHaveAttribute("type", "number"));
    expect(
      fetchMock.mock.calls.some((call) =>
        urlOf(call[0]).pathname.endsWith("/datamodels/bb-air-quality/source"),
      ),
    ).toBe(true);
  });
});

describe("typeGroups (T-5622)", () => {
  it("offers every type as granted without a grant document", () => {
    expect(typeGroups(["A", "B"], undefined, undefined)).toEqual({ granted: ["A", "B"], ungranted: [] });
  });
  it("puts the granted types first and the others apart", () => {
    expect(typeGroups(["A", "B", "C"], undefined, ["C", "A"])).toEqual({ granted: ["A", "C"], ungranted: ["B"] });
  });
  it("keeps a chosen type no list holds, once", () => {
    expect(typeGroups(["A"], "Z", ["A"])).toEqual({ granted: ["A"], ungranted: ["Z"] });
    expect(typeGroups(["A"], "A", undefined)).toEqual({ granted: ["A"], ungranted: [] });
  });
  it("offers nothing for an empty list and no choice", () => {
    expect(typeGroups([], undefined, [])).toEqual({ granted: [], ungranted: [] });
  });
});
