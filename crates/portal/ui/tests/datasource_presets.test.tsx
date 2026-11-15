/**
 * T-7076: the preset and paging fields of an http DataSource, each with its hint; Find in the
 * service, which lists what a preset can read from the server's discovery and writes the pick
 * into the form; and the connection test's page count.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ConnectionTest } from "../src/pages/datasources/ConnectionTest";
import { applied, PresetDiscovery } from "../src/pages/datasources/PresetDiscovery";
import { dataSourceSchema, httpUiSchema } from "../src/schemas/kinds";
import type { JsonSchema } from "../src/components/forms/types";
import { json, problem, renderPage } from "./page_contract";

const PROJECT = "helsinki";
const t = (key: string) => i18n.t(key);
const WFS = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the http form's preset and paging blocks", () => {
  const http = (dataSourceSchema(t, "http").properties as Record<string, JsonSchema>).http;
  const blocks = http.properties as Record<string, JsonSchema>;

  // Their help, in four locales, is checked with every DataSource field's in form_help.test.ts.
  it.each(["preset", "paging"])("titles every %s field in words", (block) => {
    const fields = blocks[block].properties as Record<string, JsonSchema>;
    expect(Object.keys(fields).length).toBeGreaterThan(5);
    for (const [name, field] of Object.entries(fields)) expect(field.title, name).not.toMatch(/^datasources\./);
  });

  it("offers the preset kinds and paging modes jc-core reads, by name", () => {
    const kinds = (blocks.preset.properties as Record<string, JsonSchema>).kind.oneOf as Array<{ const: string; title: string }>;
    expect(kinds.map((k) => k.const)).toEqual(["ckan", "arcgisFeature", "odata", "wfs", "ogcFeatures", "sensorThings"]);
    expect(kinds.every((k) => !k.title.startsWith("datasources."))).toBe(true);
    const modes = (blocks.paging.properties as Record<string, JsonSchema>).mode.oneOf as Array<{ const: string }>;
    expect(modes.map((m) => m.const)).toEqual(["page", "offset", "cursor", "link", "nextUrl"]);
  });
});

describe("httpUiSchema", () => {
  const PRESET = ["pageSize", "organization", "typeNames", "srsName"];
  const PAGING = ["param", "offsetParam", "limit", "next", "maxPages"];
  const shown = (block: Record<string, unknown>, names: string[]) => names.filter((name) => !(name in block));

  it("shows no preset or paging field before a kind or a mode is chosen", () => {
    const ui = httpUiSchema(undefined).http as Record<string, Record<string, unknown>>;
    expect(shown(ui.preset, PRESET)).toEqual([]);
    expect(shown(ui.paging, PAGING)).toEqual([]);
  });

  it("shows the fields of the chosen preset kind and hides Paging, which the preset sets", () => {
    const ui = httpUiSchema({ preset: { kind: "wfs" } }).http as Record<string, Record<string, unknown>>;
    expect(shown(ui.preset, PRESET)).toEqual(["pageSize", "typeNames", "srsName"]);
    expect(ui.paging).toEqual({ "ui:widget": "hidden" });
  });

  it("shows the fields of the chosen paging mode and the stop rules", () => {
    const ui = httpUiSchema({ paging: { mode: "offset" } }).http as Record<string, Record<string, unknown>>;
    expect(shown(ui.paging, PAGING)).toEqual(["offsetParam", "limit", "maxPages"]);
  });
});

describe("applied", () => {
  const http = { url: "https://typed.example/x?y=1", preset: { kind: "wfs", pageSize: 200 } };
  it.each([
    ["typeNames", WFS + "?service=WFS", "avoindata:Puut", { ...http, preset: { ...http.preset, typeNames: "avoindata:Puut" } }],
    ["organization", "https://data.example/api/3/action/organization_list", "bcn", { ...http, preset: { ...http.preset, organization: "bcn" } }],
    ["layer", "https://geo.example/rest/services/X/FeatureServer?f=json", "17", { ...http, url: "https://geo.example/rest/services/X/FeatureServer/17/query" }],
    ["entitySet", "https://services.odata.org/V4/Northwind/Northwind.svc", "Customers", { ...http, url: "https://services.odata.org/V4/Northwind/Northwind.svc/Customers" }],
    ["collection", "https://demo.pygeoapi.io/master/collections?f=json", "lakes", { ...http, url: "https://demo.pygeoapi.io/master/collections/lakes/items" }],
    ["thing", "https://iot.hamburg.de/v1.1/Things?$select=@iot.id,name", "3", { ...http, url: "https://iot.hamburg.de/v1.1/Things(3)/Datastreams" }],
  ] as const)("writes a %s where the source reads it", (field, url, value, expected) => {
    expect(applied(http, { field, url }, value)).toEqual(expected);
  });

  it("escapes a picked value inside the URL", () => {
    expect(applied(http, { field: "collection", url: "https://x.example/collections?f=json" }, "a b/c").url).toBe(
      "https://x.example/collections/a%20b%2Fc/items",
    );
  });
});

describe("Find in the service", () => {
  function renderFind(http: Record<string, unknown> | undefined, answer: () => Response) {
    const onUse = vi.fn();
    const asked: unknown[] = [];
    renderPage(<PresetDiscovery project={PROJECT} http={http} onUse={onUse} />, {
      path: `/projects/${PROJECT}/datasources`,
      answer: async (url, request) => {
        if (!url.pathname.endsWith("/datasource-discovery") || request.method !== "POST") return undefined;
        asked.push(await request.json());
        return answer();
      },
    });
    return { onUse, asked };
  }

  it("says what to fill in first when no preset kind or URL is in the form", async () => {
    renderFind({ url: WFS }, () => json({}));
    expect(await screen.findByText(/Choose an API under Open data API preset/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Find" })).toBeNull();
  });

  it("lists what the server found and writes the pick into the form", async () => {
    const http = { url: WFS, preset: { kind: "wfs" } };
    const { onUse, asked } = renderFind(http, () =>
      json({
        field: "typeNames",
        url: `${WFS}?service=WFS&version=2.0.0&request=GetCapabilities`,
        choices: [
          { value: "avoindata:Puut", label: "Trees" },
          { value: "avoindata:Kadut", label: "avoindata:Kadut" },
        ],
        truncated: true,
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Find" }));
    const select = await screen.findByRole("combobox", { name: /Feature type · Found 2 entries/ });
    expect(asked).toEqual([{ kind: "wfs", url: WFS }]);
    expect(screen.getByRole("option", { name: "Trees (avoindata:Puut)" })).toBeInTheDocument();
    expect(screen.getByText(/lists more than the Portal reads/)).toBeInTheDocument();
    await userEvent.selectOptions(select, "avoindata:Kadut");
    await userEvent.click(screen.getByRole("button", { name: "Use" }));
    expect(onUse).toHaveBeenCalledWith({ url: WFS, preset: { kind: "wfs", typeNames: "avoindata:Kadut" } });
    expect(screen.getByText(/Written into the form: avoindata:Kadut\./)).toBeInTheDocument();
  });

  it("says why nothing was listed, in the server's words", async () => {
    renderFind({ url: WFS, preset: { kind: "wfs" } }, () =>
      json({ field: "typeNames", url: WFS, choices: [], truncated: false, skipped: "the feed answered 404 Not Found" }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Find" }));
    expect(await screen.findByText(/Nothing was listed: the feed answered 404 Not Found/)).toBeInTheDocument();
  });

  it("says an empty listing has nothing this preset reads", async () => {
    renderFind({ url: WFS, preset: { kind: "wfs" } }, () => json({ field: "typeNames", url: WFS, choices: [], truncated: false }));
    await userEvent.click(await screen.findByRole("button", { name: "Find" }));
    expect(await screen.findByText(/lists nothing this preset can read/)).toBeInTheDocument();
  });

  it("shows a refusal's detail", async () => {
    renderFind({ url: "http://10.0.0.8/wfs", preset: { kind: "wfs" } }, () =>
      problem(403, "no grant proposes a DataSource in project helsinki"),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Find" }));
    expect(await screen.findByText(/no grant proposes a DataSource in project helsinki Ask a project administrator/)).toBeInTheDocument();
  });
});

describe("the connection test", () => {
  it("says how many pages a paged source's read walked", async () => {
    renderPage(<ConnectionTest project={PROJECT} name="lakes" />, {
      path: `/projects/${PROJECT}/datasources/lakes`,
      answer: (url, request) =>
        url.pathname.endsWith("/datasources/lakes/test") && request.method === "POST"
          ? json({ records: 25, bytes: 4200, pages: 3, sample: { id: 1 } })
          : undefined,
    });
    await userEvent.click(await screen.findByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(screen.getByTestId("datasource-test-pages")).toHaveTextContent("Read 3 pages."));
  });
});
