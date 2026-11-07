/** The city's datasets as grids over what the public endpoint answers (T-2782). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { AIR, answer, EVENTS, SCHOOLS } from "./fixtures/verejne";
import { LOCALES } from "./locales";

const s = LOCALES.sk;
const SLUG = "fq4xw2ztnbe7rcav3ms6kd5ypu";
const PORTAL = "https://portal.dev.joinedcontext.com/projects/banskabystrica";
let asked: URL[];

function show(withEndpoint = true, extra: Partial<Parameters<typeof stubClient>[1]> = {}, unnamed = false) {
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      asked.push(url);
      const type = url.searchParams.get("type");
      // An air station the feed sent without a name: the grid names it by its id.
      const rows = unnamed && type === "AirQualityObserved" ? [{ id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-verejne:x9", type }] : answer(type);
      return new Response(JSON.stringify(rows), {
        status: 200,
        headers: { "content-type": "application/json", "NGSILD-Results-Count": String(rows.length) },
      });
    }),
  );
  // The panel reads one entity through the client (keyValues), the grid its pages through fetch.
  const flat = [...EVENTS, ...SCHOOLS, ...AIR].map((entity) => ({
    id: entity.id,
    type: entity.type,
    name: (entity.name as { languageMap: { sk: string } }).languageMap.sk,
    ...("address" in entity ? { address: (entity.address as { value: string }).value } : {}),
  }));
  const client = stubClient({ entities: flat }, {
    portal: PORTAL,
    slug: withEndpoint ? SLUG : "",
    orgDomain: "banskabystrica.sk",
    space: withEndpoint ? "banskabystrica-verejne" : "elsewhere",
    transport: "origin",
    appName: "banskabystrica-data",
    language: "sk",
    ...extra,
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("the city's datasets", () => {
  it("opens on the events, read from the endpoint by type, with the city's column labels", async () => {
    show();
    expect(screen.getByRole("tab", { name: s.dataset.events })).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: s.dataset.events });
    await waitFor(() => expect(within(panel).getByText("Radvanský jarmok")).toBeInTheDocument());
    expect(within(panel).getByRole("columnheader", { name: new RegExp(s.column.eventCategory) })).toBeInTheDocument();
    expect(asked.some((url) => url.pathname.includes(`/api/endpoint/${SLUG}/`) && url.searchParams.get("type") === "Event")).toBe(true);
  });

  it("moves between datasets with the arrow keys and shows each one's grid and downloads", async () => {
    show();
    screen.getByRole("tab", { name: s.dataset.events }).focus();
    await userEvent.keyboard("{ArrowRight}");
    const schools = screen.getByRole("tab", { name: s.dataset.schools });
    expect(schools).toHaveFocus();
    expect(schools).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: s.dataset.schools });
    await waitFor(() => expect(within(panel).getByText("Základná škola, Moyzesova 18")).toBeInTheDocument());
    expect(within(panel).getByRole("link", { name: `${s.csv} ${s.dataset.schools}` })).toHaveAttribute(
      "href",
      `/api/endpoint/${SLUG}/file.csv?type=Organization&humanHeaders=true`,
    );
    await userEvent.keyboard("{End}");
    expect(screen.getByRole("tab", { name: s.dataset.air })).toHaveFocus();
    await userEvent.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: s.dataset.events })).toHaveFocus();
  });

  it("opens a row in the SDK's panel from its name, which links to the Portal and offers no Edit", async () => {
    show();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: `${s.grid.openRow}: Radvanský jarmok` }));
    const panel = await screen.findByRole("dialog", { name: "Radvanský jarmok" });
    const link = await within(panel).findByRole("link", { name: "Otvoriť v Portáli" });
    expect(link).toHaveAttribute("href", `${PORTAL}/explore?space=banskabystrica-verejne&entityId=${encodeURIComponent(EVENTS[0].id)}`);
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    // Following it leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    link.addEventListener("click", (event) => event.preventDefault());
    await user.click(link);
    await user.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // A school opens as a school.
    await user.click(screen.getByRole("tab", { name: s.dataset.schools }));
    await user.click(await screen.findByRole("button", { name: `${s.grid.openRow}: Základná škola, Moyzesova 18` }));
    expect(await screen.findByRole("dialog", { name: "Základná škola, Moyzesova 18" })).toBeInTheDocument();
  });

  it("on every dataset: opens each row in the panel, and offers the endpoint's own downloads", async () => {
    show();
    const user = userEvent.setup();
    for (const dataset of ["events", "schools", "air"] as const) {
      await user.click(screen.getByRole("tab", { name: s.dataset[dataset] }));
      const panel = screen.getByRole("tabpanel", { name: s.dataset[dataset] });
      for (const row of await within(panel).findAllByRole("button", { name: new RegExp(`^${s.grid.openRow}: `) })) {
        await user.click(row);
        const name = (row.getAttribute("aria-label") ?? "").slice(`${s.grid.openRow}: `.length);
        expect(await screen.findByRole("dialog", { name })).toBeInTheDocument();
        await user.keyboard("{Escape}");
      }
      expect(panel.querySelectorAll(".downloads a"), "one download: the gateway serves no GeoJSON file (T-4597, T-4603)").toHaveLength(1);
      for (const link of within(panel).getAllByRole("link", { name: new RegExp(`^${s.csv} `) })) {
        link.addEventListener("click", (event) => event.preventDefault());
        await user.click(link);
        expect(link).toHaveAttribute("download");
      }
    }
  }, 15_000);

  it("finds its endpoint among several the Portal hands it, and speaks English when asked", async () => {
    show(false, { endpoints: [
        { slug: "other", space: "elsewhere", name: "other", types: [] },
        { slug: SLUG, space: "banskabystrica-verejne", name: "verejne", types: ["Event", "Organization", "AirQualityObserved"] },
      ],
      language: "en",
    });
    const en = LOCALES.en;
    expect(screen.getByRole("heading", { level: 1, name: en.title })).toBeInTheDocument();
    await waitFor(() => expect(asked.some((url) => url.pathname.includes(`/api/endpoint/${SLUG}/`))).toBe(true));
    const user = userEvent.setup();
    for (const dataset of ["schools", "air", "events"] as const) {
      await user.click(screen.getByRole("tab", { name: en.dataset[dataset] }));
      expect(screen.getByRole("tabpanel", { name: en.dataset[dataset] })).toBeInTheDocument();
      for (const link of screen.getAllByRole("link", { name: new RegExp(`^${en.csv} `) })) {
        link.addEventListener("click", (event) => event.preventDefault());
        await user.click(link);
        expect(link.getAttribute("href")).toContain(`/api/endpoint/${SLUG}/file.`);
      }
    }
  });

  it("falls back to Slovak for a language it does not speak, its panel too", async () => {
    show(true, { language: "de" });
    expect(screen.getByRole("heading", { level: 1, name: s.title })).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: `${s.grid.openRow}: Radvanský jarmok` }));
    const panel = await screen.findByRole("dialog", { name: "Radvanský jarmok" });
    expect(within(panel).getByRole("button", { name: "Zavrieť" })).toBeInTheDocument();
  });

  it("moves back with ArrowLeft, wraps around, and ignores any other key", async () => {
    show();
    screen.getByRole("tab", { name: s.dataset.events }).focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: s.dataset.air })).toHaveFocus();
    await userEvent.keyboard("a");
    expect(screen.getByRole("tab", { name: s.dataset.air })).toHaveAttribute("aria-selected", "true");
  });

  it("names a row without a name by its id, and opens it so", async () => {
    show(true, {}, true);
    await userEvent.click(screen.getByRole("tab", { name: s.dataset.air }));
    // The SDK grid calls a row named by its id by the id's last part, as the Portal does (T-5619).
    const air = screen.getByRole("tabpanel", { name: s.dataset.air });
    const open = await within(air).findByRole("button", { name: `${s.grid.openRow}: x9` });
    expect(open).toHaveAttribute("title", "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-verejne:x9");
  });

  it("says it has nothing to read when the app has no endpoint of the public space", () => {
    show(false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("names its sources and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByText("Radvanský jarmok")).toBeInTheDocument());
    expect(screen.getByText(s.source)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
