/** The Žilina dashboard over what the public endpoint of `zilina-kpi` answers (T-3140). */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { answer, INDICATORS } from "./fixtures/kpi";
import { LOCALES } from "./locales";

const s = LOCALES.sk;
const SLUG = "ricvfpfxtaxeyzf5c3b3es7ymc6unh3o";

function show(body: () => Response, withEndpoint = true, language = "sk") {
  vi.stubGlobal("fetch", vi.fn(async () => body()));
  // The cards read through `fetch` above; the entity panel reads one indicator through the client (SDK-40).
  const client = stubClient({ entities: answer("KeyPerformanceIndicator") as Row[] }, {
    portal: "https://portal.zilina.sk/projects/zilina",
    slug: withEndpoint ? SLUG : "",
    orgDomain: "zilina.sk",
    space: withEndpoint ? "zilina-kpi" : "elsewhere",
    transport: "origin",
    appName: "zilina-ukazovatele",
    language,
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the Žilina dashboard", () => {
  it("shows the six indicators in their order, each with its value, unit and window", async () => {
    show(() => json(answer("KeyPerformanceIndicator")));
    const cards = await screen.findAllByRole("article");
    expect(cards.map((card) => within(card).getByRole("heading").textContent)).toEqual([
      s.label["obyvatelstvo-stav"],
      s.label["celkovy-prirastok"],
      s.label["priemerny-vek"],
      s.label["index-starnutia"],
      s.label.uchadzaci,
      s.label["navstevnici-rok"],
    ]);
    const population = screen.getByRole("article", { name: s.label["obyvatelstvo-stav"] });
    expect(population).toHaveTextContent(/79\s617 osôb/);
    expect(population).toHaveTextContent("2. štvrťrok 2026");
    expect(screen.getByRole("article", { name: s.label["celkovy-prirastok"] })).toHaveTextContent(/-384 osôb|−384 osôb/);
    expect(screen.getByRole("article", { name: s.label["navstevnici-rok"] })).toHaveTextContent("rok 2025");
    expect(screen.getByText(s.noThreshold)).toBeInTheDocument();
  });

  // SDK-40, AP-140: a public App opens an indicator in the panel, which links to the Portal and edits nothing.
  it("opens every indicator in the entity panel, with a Portal link and no Edit", async () => {
    show(() => json(answer("KeyPerformanceIndicator")));
    await screen.findAllByRole("article");
    const user = userEvent.setup();
    for (const key of Object.keys(s.label) as Array<keyof typeof s.label>) {
      await user.click(screen.getByRole("button", { name: s.label[key] }));
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    const panel = screen.getByRole("dialog");
    await user.click(await within(panel).findByRole("link", { name: "Otvoriť v Portáli" }));
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    await user.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("speaks English to an English reader, and opens an indicator there too", async () => {
    const en = LOCALES.en;
    show(() => json(answer("KeyPerformanceIndicator")), true, "en");
    expect(screen.getByRole("heading", { level: 1, name: en.title })).toBeInTheDocument();
    await screen.findAllByRole("article");
    const user = userEvent.setup();
    for (const key of Object.keys(en.label) as Array<keyof typeof en.label>) {
      await user.click(screen.getByRole("button", { name: en.label[key] }));
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    const panel = screen.getByRole("dialog");
    await user.click(await within(panel).findByRole("link", { name: "Open in the Portal" }));
    await user.click(within(panel).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says a value outside its unit by its code, a window of days, a run time it cannot read, and no headline without the population", async () => {
    const rows = structuredClone(INDICATORS) as unknown as Array<Record<string, Record<string, unknown>>>;
    const by = (name: string) => rows.find((row) => row.name.value === name) as Record<string, Record<string, unknown>>;
    by("priemerny-vek-mesto").currentValue.unitCode = "C62";
    by("uchadzaci-mesto").calculationPeriod.value = { start: "2026-08-01T00:00:00Z", end: "2026-08-31T23:59:59Z" };
    by("uchadzaci-mesto").updatedAt = { type: "Property", value: "yesterday" };
    delete by("index-starnutia-mesto").calculationPeriod;
    delete by("index-starnutia-mesto").updatedAt;
    show(() => json(rows.filter((row) => row.name.value !== "obyvatelstvo-stav-mesto")), true, undefined as unknown as string);
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(5);
    expect(document.querySelector(".headline")).toBeNull();
    expect(screen.getByRole("article", { name: s.label["priemerny-vek"] })).toHaveTextContent("44,03 C62");
    const jobless = screen.getByRole("article", { name: s.label.uchadzaci });
    expect(jobless).toHaveTextContent("2026-08-01 – 2026-08-31");
    expect(jobless).toHaveTextContent(s.computed("yesterday"));
    expect(screen.getByRole("article", { name: s.label["index-starnutia"] })).not.toHaveTextContent(s.window);
  });

  it("reads the window and the run in English", async () => {
    const en = LOCALES.en;
    show(() => json(answer("KeyPerformanceIndicator")), true, "en");
    expect(await screen.findByRole("article", { name: en.label["obyvatelstvo-stav"] })).toHaveTextContent("Q2 2026");
    expect(screen.getByRole("article", { name: en.label["navstevnici-rok"] })).toHaveTextContent("year 2025");
    expect(screen.getAllByText(/^Computed /).length).toBeGreaterThan(0);
  });

  it("says a failure that is not even an error as it came, and keeps quiet once the page is gone", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw "the proxy hung up";
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "zilina.sk", space: "zilina-kpi", transport: "origin", appName: "zilina-ukazovatele", language: "en" });
    const view = render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(LOCALES.en.refused("the proxy hung up"));
    view.unmount();
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        await held;
        return json([]);
      }),
    );
    const again = render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    again.unmount();
    release();
    await held;
  });

  it("reads through the endpoint the Portal lists for the space, beside the App's others", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => json(answer("KeyPerformanceIndicator"))));
    const client = stubClient({ entities: answer("KeyPerformanceIndicator") as Row[] }, {
      portal: "https://portal.zilina.sk/projects/zilina",
      slug: "",
      orgDomain: "zilina.sk",
      space: "zilina",
      transport: "origin",
      appName: "zilina-ukazovatele",
      language: "sk",
      endpoints: [
        { space: "zilina", slug: "other", name: "app-zilina-ukazovatele-zilina", types: [] },
        { space: "zilina-kpi", slug: SLUG, name: "app-zilina-ukazovatele", types: ["KeyPerformanceIndicator"] },
      ],
    });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    await screen.findAllByRole("article");
    expect(vi.mocked(fetch).mock.calls[0][0]).toContain(`/api/endpoint/${SLUG}/`);
    await userEvent.click(screen.getByRole("button", { name: s.label.uchadzaci }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("says when the city has published no indicator", async () => {
    show(() => json([]));
    expect(await screen.findByText(s.none)).toBeInTheDocument();
  });

  it("says not measured where the pipeline found no row, and drops what it cannot place", async () => {
    const rows = structuredClone(INDICATORS) as Array<Record<string, unknown>>;
    rows[0].currentValue = { type: "Property", value: "not measured" };
    rows.push({ ...rows[1], id: String(rows[1].id).replace(":zilina-kpi:", ":bbsk-kpi:") });
    show(() => json(rows));
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(6);
    expect(screen.getByText(s.notMeasured)).toBeInTheDocument();
  });

  it("says why the endpoint refused, and that it has nothing to read without one", async () => {
    show(() => new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "no policy" }), {
      status: 403,
      headers: { "content-type": "application/problem+json" },
    }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no policy");
  });

  it("has nothing to read without an endpoint of the indicator space", () => {
    show(() => json([]), false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
  });

  it("names its source and has nothing axe finds", async () => {
    const { container } = show(() => json(answer("KeyPerformanceIndicator")));
    await screen.findAllByRole("article");
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
