/**
 * The Žilina records over what the app's three endpoints answer (T-3140). `fetch` is stubbed, so
 * every case goes through the SDK's own endpoint source and grid.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { answer, MONUMENTS } from "./fixtures/data";
import { LOCALES } from "./locales";

const s = LOCALES.sk;
const ENDPOINTS = [
  { name: "app-zilina-zaznamy", slug: "verejne-slug", space: "zilina-verejne", types: ["PointOfInterest", "GtfsStop", "AirQualityObserved"] },
  { name: "app-zilina-zaznamy-uniza", slug: "uniza-slug", space: "zilina-uniza", types: ["Article"] },
];

/** Every row the endpoints hold, for the entity panel's reads through the client (SDK-40). */
const ROWS = ["PointOfInterest", "GtfsStop", "AirQualityObserved", "Article"].flatMap((type) => answer(type)) as Row[];

/** What the endpoints answer from here on, where a test answers otherwise than the fixture. */
let answering: ((path: string) => Promise<Response>) | null = null;

function show(language = "sk") {
  const calls: string[] = [];
  answering = null;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      calls.push(path);
      if (answering) return answering(path);
      const type = new URL(path, "http://portal.test").searchParams.get("type");
      return new Response(JSON.stringify(answer(type)), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
  const client = stubClient({ entities: ROWS }, {
    portal: "https://portal.zilina.sk/projects/zilina",
    slug: "verejne-slug",
    orgDomain: "zilina.sk",
    space: "zilina-verejne",
    endpoints: ENDPOINTS,
    transport: "origin",
    appName: "zilina-zaznamy",
    language,
  } as never);
  const view = render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return { ...view, calls };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const choose = (name: string) => userEvent.click(within(screen.getByRole("navigation")).getByRole("button", { name: new RegExp(name) }));

describe("the Žilina records", () => {
  it("opens on the monuments with the model's columns in Slovak", async () => {
    show();
    const table = await screen.findByRole("grid");
    await waitFor(() => expect(within(table).getByText("Trojičný stĺp")).toBeInTheDocument());
    for (const column of ["Objekt", "Sloh", "Vznik", "Katastrálne územie", "Forma vlastníctva"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(column) })).toBeInTheDocument();
    }
    expect(screen.getByText(s.readOnly)).toBeInTheDocument();
  });

  it("reads each dataset through the endpoint of its own space", async () => {
    const { calls } = show();
    await choose(s.name.works);
    await waitFor(() => expect(calls.some((call) => call.includes("/api/endpoint/uniza-slug/") && call.includes("type=Article"))).toBe(true));
    expect(await screen.findByRole("heading", { level: 2, name: s.name.works })).toBeInTheDocument();
    await choose(s.name.stations);
    await waitFor(() => expect(within(screen.getByRole("grid")).getByText("Brodno")).toBeInTheDocument());
    expect(calls.filter((call) => call.includes("type=GtfsStop")).every((call) => call.includes("/api/endpoint/verejne-slug/"))).toBe(true);
  });

  it("says so where the app has no endpoint of a dataset's space", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("[]", { status: 200, headers: { "content-type": "application/json" } })));
    const client = stubClient(undefined, { slug: "verejne-slug", orgDomain: "zilina.sk", space: "zilina-verejne", endpoints: [ENDPOINTS[0]], transport: "origin", appName: "zilina-zaznamy", language: "sk" } as never);
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    await choose(s.name.works);
    expect(screen.getByRole("alert")).toHaveTextContent(s.noEndpoint(s.name.works));
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("downloads the whole dataset and says how many rows", async () => {
    const created = vi.fn(() => "blob:x");
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: created, revokeObjectURL: vi.fn() }));
    show();
    await screen.findByRole("grid");
    await userEvent.click(screen.getByRole("button", { name: s.exportCsv }));
    await waitFor(() => expect(screen.getByText(s.exported(5))).toBeInTheDocument());
    expect(created).toHaveBeenCalledTimes(1);
  });

  // SDK-40, AP-140: a row opened from the grid shows in the shell's panel, linked to the Portal, no Edit.
  it("opens a row of each dataset in the entity panel, with a Portal link and no Edit", async () => {
    show();
    const user = userEvent.setup();
    for (const [dataset, name] of [
      ["monuments", "Trojičný stĺp"],
      ["stations", "Brodno"],
      ["air", "Stanica SK0020A, mestské pozadie"],
    ] as const) {
      await choose(s.name[dataset]);
      await user.click(await screen.findByRole("button", { name: `Otvoriť: ${name}` }));
      const panel = await screen.findByRole("dialog");
      await user.click(await within(panel).findByRole("link", { name: "Otvoriť v Portáli" }));
      expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
      await user.click(within(panel).getByRole("button", { name: "Zavrieť" }));
      expect(screen.queryByRole("dialog")).toBeNull();
    }
    await choose(s.name.works);
    const grid = await screen.findByRole("grid");
    await waitFor(() => expect(within(grid).getAllByRole("button", { name: /^Otvoriť: / }).length).toBeGreaterThan(0));
    await user.click(within(grid).getAllByRole("button", { name: /^Otvoriť: / })[0]);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await choose(s.name.monuments);
  });

  it("speaks English to an English reader, every dataset and its download too", async () => {
    const en = LOCALES.en;
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: vi.fn(() => "blob:x"), revokeObjectURL: vi.fn() }));
    show("en");
    expect(screen.getByRole("heading", { level: 1, name: en.title })).toBeInTheDocument();
    const user = userEvent.setup();
    for (const dataset of ["stations", "air", "works", "monuments"] as const) {
      await user.click(within(screen.getByRole("navigation")).getByRole("button", { name: new RegExp(en.name[dataset]) }));
      expect(await screen.findByRole("heading", { level: 2, name: en.name[dataset] })).toBeInTheDocument();
    }
    await user.click(screen.getByRole("button", { name: en.exportCsv }));
    await waitFor(() => expect(screen.getByText(en.exported(5))).toBeInTheDocument());
    await user.click(await screen.findByRole("button", { name: "Open: Trojičný stĺp" }));
    const panel = await screen.findByRole("dialog");
    await user.click(await within(panel).findByRole("link", { name: "Open in the Portal" }));
    await user.click(within(panel).getByRole("button", { name: "Close" }));
  });

  it("says the download is busy while it runs, and why it failed, in the endpoint's words or as thrown", async () => {
    show();
    await screen.findByRole("grid");
    let fail: (cause: unknown) => void = () => {};
    answering = () => new Promise<Response>((_, reject) => (fail = reject));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: s.exportCsv }));
    expect(screen.getByRole("button", { name: s.exportCsv })).toBeDisabled();
    expect(screen.getByText(s.exporting)).toBeInTheDocument();
    fail(new TypeError("the network is down"));
    expect(await screen.findByText(s.exportFailed("the network is down"))).toBeInTheDocument();
    answering = async () => {
      throw "the proxy hung up";
    };
    await user.click(screen.getByRole("button", { name: s.exportCsv }));
    expect(await screen.findByText(s.exportFailed("the proxy hung up"))).toBeInTheDocument();
    // Another dataset starts with no download said.
    await choose(s.name.stations);
    expect(screen.queryByText(/Súbor sa nepodarilo/)).toBeNull();
  });

  it("says when the download stopped at its ceiling", async () => {
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: vi.fn(() => "blob:x"), revokeObjectURL: vi.fn() }));
    show();
    await screen.findByRole("grid");
    const full = Array.from({ length: 500 }, (_, i) => ({ ...MONUMENTS[1], id: `urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:${i}` }));
    answering = async () => new Response(JSON.stringify(full), { status: 200, headers: { "content-type": "application/json" } });
    await userEvent.click(screen.getByRole("button", { name: s.exportCsv }));
    expect(await screen.findByText(s.exportCut(5000))).toBeInTheDocument();
  });

  it("names whose data each dataset is and has nothing axe finds", async () => {
    const { container } = show();
    await screen.findByRole("grid");
    expect(screen.getByText(s.licence.monuments)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
