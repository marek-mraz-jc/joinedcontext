import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider, ProblemError } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { reachApi } from "./server";
import type { ReachApi } from "./server";
import { AnalyserContext } from "./analysis";
import type { AnalysisInput } from "./analysis";
import { memory, STEWARD, VIEWER } from "./fixtures/memory";
import { NETWORK_ROWS } from "./fixtures/network";
import { HISTORY } from "./fixtures/vehicles";
import { inProcess } from "./test-analyser";
import { sweep } from "./testing/sweep";
import { Map as FakeMap } from "./testing/maplibre";

vi.mock("maplibre-gl", () => import("./testing/maplibre"));
const went = vi.hoisted(() => [] as string[]);
vi.mock("./go", () => ({ go: (url: string) => went.push(url) }));

const READ = { permissions: [{ resource: { type: "Vehicle" }, actions: ["retrieveTemporal"], attributes: "*" as const }], prohibitions: [] };

function client(temporal: unknown[] = HISTORY, entities: NonNullable<Parameters<typeof stubClient>[0]>["entities"] = []) {
  return stubClient(
    { entities, temporal: temporal as { id: string; type: string }[], access: READ },
    { appName: "transit-reach", portal: "https://portal.hel.fi/projects/helsinki" },
  );
}

function show(c = client(), server: ReachApi = memory(null)) {
  render(
    <JcProvider client={c}>
      <AnalyserContext.Provider value={inProcess}>
        <App server={server} />
      </AnalyserContext.Provider>
    </JcProvider>,
  );
  return c;
}

const FI_SUMMARY = /^Saavutettava alue tästä pisteestä: 10 min \d+,\d km², 20 min \d+,\d km², 30 min \d+,\d km²\. Pysäkkejä 30 minuutissa: 4\/4\.$/;

describe("transit-reach", () => {
  beforeEach(() => window.history.replaceState(null, "", "/?lang=fi"));
  afterEach(() => {
    vi.restoreAllMocks();
    window.history.replaceState(null, "", "/");
    FakeMap.built.length = 0;
  });

  // AP-04: read through the app's own endpoint only, never written to; the first screen answers
  // how far one gets from Rautatientori without a click.
  it("answers how far one gets from Rautatientori on arrival, in Finnish, read only through the app's endpoint", async () => {
    const c = show();
    expect(screen.getByRole("heading", { level: 1, name: "Kuinka pitkälle pääsen joukkoliikenteellä" })).toBeInTheDocument();
    expect(await screen.findByText(FI_SUMMARY)).toBeInTheDocument();
    expect(screen.getByText(/^Pysäkit on päätelty paikoista, joissa 5 ajoneuvoa seisoi 7\.10\.2030 klo 09\.00–7\.10\.2030 klo 09\.\d\d\./)).toBeInTheDocument();
    const reached = screen.getByRole("list", { name: "Pysäkit 30 minuutissa" });
    expect(within(reached).getAllByRole("listitem").map((li) => li.querySelector("span")?.textContent)).toEqual([
      "0,0 min lähtöpisteestä",
      "9,0 min lähtöpisteestä",
      "13,0 min lähtöpisteestä",
      "22,0 min lähtöpisteestä",
    ]);
    expect(screen.getByRole("application", { name: /^Kartta: saavutettava alue\./ })).toBeInTheDocument();
    expect(c.transport.calls.every((call) => call.method === "GET" && call.path.includes("/api/endpoint/"))).toBe(true);
    // HSL's stops and lines are read first; this space has none, so the vehicles' history follows.
    expect(c.transport.calls.filter((call) => /type=(GtfsStop|PublicTransportRoute)/.test(call.path))).toHaveLength(2);
    const temporal = c.transport.calls.find((call) => call.path.includes("/temporal/entities"));
    expect(temporal?.path).toContain("attrs=location%2Cspeed%2Croute");
  });

  it("starts from a stop picked in the list or from the reached stops, and the address carries the point", async () => {
    show();
    await screen.findByText(FI_SUMMARY);
    const user = userEvent.setup();
    const select = screen.getByRole("combobox", { name: "Lähde pysäkiltä" });
    const options = within(select).getAllByRole("option");
    const north = options.find((o) => o.textContent?.includes("4570") && !o.textContent.includes("550"));
    await user.selectOptions(select, north!);
    expect(window.location.search).toMatch(/at=25\.01\d+%2C60\.19\d+/);
    expect(await screen.findByText(/Pysäkkejä 30 minuutissa: 1\/4\.$/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Takaisin Rautatientorille" }));
    expect(await screen.findByText(FI_SUMMARY)).toBeInTheDocument();
    expect(window.location.search).toBe("?lang=fi");
    const names = screen.getAllByRole("button", { name: /: lähde tästä$/ }).map((b) => b.textContent ?? "");
    for (const name of names) {
      await user.click(await screen.findByRole("button", { name: `${name}: lähde tästä` }));
      expect(window.location.search).toContain("at=");
      await user.click(screen.getByRole("button", { name: "Takaisin Rautatientorille" }));
    }
  });

  // Found by worker-4 (chyby.md): before the history read began, the page analysed an empty history
  // and a walking-only answer flashed before the vehicles' one.
  it("never answers from an empty history before the history has been read", async () => {
    const asked: AnalysisInput[] = [];
    const watching = (input: AnalysisInput) => {
      asked.push(input);
      return inProcess(input);
    };
    render(
      <JcProvider client={client()}>
        <AnalyserContext.Provider value={watching}>
          <App />
        </AnalyserContext.Provider>
      </JcProvider>,
    );
    await screen.findByText(FI_SUMMARY);
    expect(asked.length).toBeGreaterThan(0);
    expect(asked.filter((input) => input.vehicles.length === 0)).toEqual([]);
  });

  it("reads the history window the address names and asks again for another", async () => {
    window.history.replaceState(null, "", "/?lang=en&hours=6");
    const c = show();
    await screen.findByText(/^Reachable from this point: /);
    expect(screen.getByRole("combobox", { name: "Vehicle history" })).toHaveValue("6");
    await userEvent.setup().selectOptions(screen.getByRole("combobox", { name: "Vehicle history" }), "1");
    expect(window.location.search).toContain("hours=1");
    expect(c.transport.calls.filter((call) => call.path.includes("/temporal/entities"))).toHaveLength(2);
  });

  it("answers with walking alone, and says why, when the history is refused", async () => {
    const c = client();
    c.temporal.list = async () => {
      throw new ProblemError(403, { title: "Forbidden", status: 403 });
    };
    show(c);
    expect(await screen.findByRole("alert")).toHaveTextContent("Ajoneuvojen historiaa ei voitu lukea (HTTP 403)");
    expect(await screen.findByText(/Pysäkkejä 30 minuutissa: 0\/0\.$/)).toBeInTheDocument();
    expect(screen.getByText("Ajoneuvoista ei ole historiaa: alue on pelkkä kävelymatka.")).toBeInTheDocument();
    expect(screen.getByText("Ei pysäkkejä 30 minuutin sisällä.")).toBeInTheDocument();
  });

  it("switches to English and keeps the language in the address", async () => {
    show();
    await screen.findByText(FI_SUMMARY);
    await userEvent.setup().selectOptions(screen.getByRole("combobox", { name: "Kieli" }), "en");
    expect(screen.getByRole("heading", { level: 1, name: "How far can I get by transit" })).toBeInTheDocument();
    expect(await screen.findByText(/^Reachable from this point: 10 min \d+\.\d km², /)).toBeInTheDocument();
    expect(document.documentElement.lang).toBe("en");
  });

  // T-3356: with HSL's stops and lines in the space, the answer rides the whole network and names
  // the stops; the vehicles' history is not read at all.
  it("rides HSL's network when the space holds it, names the stops, and reads no vehicle history", async () => {
    const c = show(client(HISTORY, NETWORK_ROWS));
    expect(await screen.findByText(/^Pysäkit ja linjat HSL:n rekistereistä \(4 pysäkkiä, 2 linjaversiota\)\./)).toBeInTheDocument();
    const reached = screen.getByRole("list", { name: "Pysäkit 30 minuutissa" });
    expect(within(reached).getAllByRole("button", { name: /lähde tästä/ }).map((b) => b.textContent)).toEqual([
      "Rautatientori (H0019): linjat M1",
      "Kaisaniemi (H0012): linjat 550, M1",
      "Hakaniemi (H0026): linjat M1",
      "Kaisaniemenranta: linjat 550",
    ]);
    expect(screen.queryByRole("combobox", { name: "Ajoneuvojen historia" })).toBeNull();
    expect(c.transport.calls.some((call) => call.path.includes("/temporal/entities"))).toBe(false);
  });

  // T-3349: from an HSL stop, the areas the App's server keeps, through its own API.
  it("keeps the start stop's areas on the server and downloads them, a stop of HSL's network only", async () => {
    const fetch = vi.fn(async (url: string, _init?: RequestInit) =>
      new Response(url.endsWith("/me") ? "null" :
        JSON.stringify({ version: "v1", stop: decodeURIComponent(url.split("stop=")[1] ?? ""), bands: [{ minutes: 10, areaKm2: 0.5, stops: 1 }], cached: false, stale: false, url: "https://store.example/t.geojson" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetch);
    try {
      show(client(HISTORY, NETWORK_ROWS), reachApi("/apps/transit-reach/api"));
      const user = userEvent.setup();
      const keep = await screen.findByRole("button", { name: "Lataa alueet lähtöpysäkiltä (GeoJSON)" });
      expect(keep).toBeDisabled();
      const reached = await screen.findByRole("list", { name: "Pysäkit 30 minuutissa" });
      await user.click(within(reached).getAllByRole("button", { name: /: lähde tästä$/ })[1]);
      expect(await screen.findByText(/^Lähtöpysäkki: Kaisaniemi \(H0012\)/)).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Lataa alueet lähtöpysäkiltä (GeoJSON)" }));
      expect(await screen.findByText("GeoJSON-tiedosto ladataan.")).toBeInTheDocument();
      expect(fetch.mock.calls.map(([url]) => url).find((url) => url.includes("/reach?"))).toMatch(/^\/apps\/transit-reach\/api\/reach\?stop=urn%3Angsi-ld%3AGtfsStop%3A/);
      expect(went.at(-1)).toBe("https://store.example/t.geojson");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("says HSL's stops could not be read, and answers from the vehicles instead", async () => {
    const c = client(HISTORY, NETWORK_ROWS);
    const list = c.entities.list.bind(c.entities);
    c.entities.list = (async (type: string, query?: Parameters<typeof list>[1]) => {
      if (type === "GtfsStop") throw new ProblemError(502, { title: "Bad Gateway", status: 502 });
      return list(type, query);
    }) as typeof c.entities.list;
    show(c);
    expect(await screen.findByText(/HSL:n pysäkkejä ja linjoja ei voitu lukea \(HTTP 502\)/)).toBeInTheDocument();
    expect(await screen.findByText(FI_SUMMARY)).toBeInTheDocument();
  });

  // SDK-40, AP-140: an HSL stop reached opens in the entity panel, with a Portal link and no Edit.
  it("opens every reached HSL stop in the entity panel, with a Portal link and no Edit", async () => {
    show(client(HISTORY, NETWORK_ROWS));
    const reached = await screen.findByRole("list", { name: "Pysäkit 30 minuutissa" });
    const user = userEvent.setup();
    for (const button of within(reached).getAllByRole("button", { name: /: tiedot$/ })) {
      await user.click(button);
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    const panel = screen.getByRole("dialog");
    const link = await within(panel).findByRole("link", { name: "Avaa portaalissa" });
    await user.click(link);
    expect(within(panel).queryByRole("button", { name: "Muokkaa" })).toBeNull();
    await user.click(within(panel).getByRole("button", { name: "Sulje" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("starts from every reached stop in turn, in either language, and from a point clicked on the map", async () => {
    show(client(HISTORY, NETWORK_ROWS));
    const user = userEvent.setup();
    const reached = await screen.findByRole("list", { name: "Pysäkit 30 minuutissa" });
    const names = within(reached).getAllByRole("button", { name: /: lähde tästä$/ }).map((b) => b.getAttribute("aria-label") ?? b.textContent ?? "");
    for (const name of names) {
      await user.click(await screen.findByRole("button", { name }));
      if (screen.queryByRole("button", { name: "Takaisin Rautatientorille" })) await user.click(screen.getByRole("button", { name: "Takaisin Rautatientorille" }));
    }
    const map = FakeMap.built.at(-1) as FakeMap;
    await vi.waitFor(() => expect(map.sources.cells).toBeDefined());
    map.under = [];
    act(() => map.fire("click", { point: { x: 1, y: 1 }, lngLat: { lng: 24.951234567, lat: 60.181234567 } }));
    expect(window.location.search).toContain("at=24.95123%2C60.18123");
  });

  it("starts from each reached stop in English, and from a stop picked in the English list", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    show();
    const user = userEvent.setup();
    await screen.findByText(/^Reachable from this point: /);
    const reached = await screen.findByRole("list", { name: /Stops within 30 minutes/ });
    const names = within(reached).getAllByRole("button", { name: /: start from here$/ }).map((b) => b.getAttribute("aria-label") ?? b.textContent ?? "");
    for (const name of names) {
      await user.click(await screen.findByRole("button", { name }));
      if (screen.queryByRole("button", { name: "Back to Rautatientori" })) await user.click(screen.getByRole("button", { name: "Back to Rautatientori" }));
    }
    const select = await screen.findByRole("combobox", { name: "Start from a stop" });
    await user.selectOptions(select, within(select).getAllByRole("option")[1]);
    expect(window.location.search).toContain("at=");
    await user.selectOptions(select, "");
    expect(window.location.search).toContain("at=");
  });

  it("asks for another history window in Finnish, and switches back to Finnish", async () => {
    window.history.replaceState(null, "", "/?lang=fi&hours=6");
    const c = show();
    await screen.findByText(FI_SUMMARY);
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole("combobox", { name: "Ajoneuvojen historia" }), "1");
    expect(c.transport.calls.filter((call) => call.path.includes("/temporal/entities"))).toHaveLength(2);
    await user.selectOptions(screen.getByRole("combobox", { name: "Kieli" }), "sv");
    expect(await screen.findByRole("heading", { level: 1, name: "Hur långt kommer jag med kollektivtrafik" })).toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: "Språk" }), "fi");
    expect(document.documentElement.lang).toBe("fi");
    expect(window.location.search).toContain("lang=fi");
  });

  it("switches language even where the address cannot be written, and takes the browser's language", async () => {
    window.history.replaceState(null, "", "/");
    vi.spyOn(navigator, "languages", "get").mockReturnValue(undefined as unknown as readonly string[]);
    vi.spyOn(navigator, "language", "get").mockReturnValue("en-GB");
    show();
    await screen.findByText(/^Reachable from this point: /);
    vi.spyOn(window.history, "replaceState").mockImplementation(() => {
      throw new DOMException("sandboxed", "SecurityError");
    });
    await userEvent.setup().selectOptions(screen.getByRole("combobox", { name: "Language" }), "fi");
    expect(screen.getByRole("heading", { level: 1, name: "Kuinka pitkälle pääsen joukkoliikenteellä" })).toBeInTheDocument();
  });

  it("tries the stops and the history again after a failure that is no HTTP answer", async () => {
    const c = client(HISTORY, NETWORK_ROWS);
    const list = c.entities.list.bind(c.entities);
    const temporal = c.temporal.list.bind(c.temporal);
    let failing = true;
    c.entities.list = (async (type: string, query?: Parameters<typeof list>[1]) => {
      if (failing) throw "network down";
      return list(type, query);
    }) as typeof c.entities.list;
    c.temporal.list = (async (...args: Parameters<typeof temporal>) => {
      if (failing) throw new Error("socket closed");
      return temporal(...args);
    }) as typeof c.temporal.list;
    show(c);
    const user = userEvent.setup();
    expect(await screen.findByText(/HSL:n pysäkkejä ja linjoja ei voitu lukea \(HTTP 0\)/)).toBeInTheDocument();
    expect(await screen.findByText(/Ajoneuvojen historiaa ei voitu lukea \(HTTP 0\)/)).toBeInTheDocument();
    failing = false;
    for (const retry of screen.getAllByRole("button", { name: "Yritä uudelleen" })) await user.click(retry);
    expect(await screen.findByText(/^Pysäkit ja linjat HSL:n rekistereistä/)).toBeInTheDocument();
  });

  // T-3826: who reads the page stands beside their name; Origins and History are a page each.
  it("names the reader's role and opens the origins and the history in every language", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    show(client(), memory(STEWARD));
    expect(await screen.findByLabelText("Your role: Steward")).toBeInTheDocument();
    expect(screen.getByText("Sami Steward")).toBeInTheDocument();
    const user = userEvent.setup();
    for (const [language, pages] of [
      ["en", ["Reach", "Origins", "History"]],
      ["fi", ["Saavutettavuus", "Lähtöpisteet", "Historia"]],
      ["sv", ["Räckvidd", "Startpunkter", "Historik"]],
    ] as const) {
      await user.selectOptions(screen.getByRole("combobox", { name: /^(Language|Kieli)$/ }), language);
      // Every control of the map page (an origin kept from it), then of the origins, in this language.
      for (const page of pages) {
        await user.click(await screen.findByRole("button", { name: page }));
        expect(screen.getByRole("button", { name: page })).toHaveAttribute("aria-current", "page");
        if (page === pages[0]) await screen.findByRole("list", { name: /^(Stops within|Pysäkit|Hållplatser inom) 30/ });
        if (page === pages[1]) await screen.findByRole("table");
        if (page !== pages[2]) await sweep(screen.getByRole("region", { name: page }));
        else await user.click(await screen.findByRole("button", { name: /^(Refresh|Päivitä|Uppdatera)$/ }));
      }
    }
  });

  it("says a viewer is a viewer, and a signed-out reader that they are not signed in", async () => {
    window.history.replaceState(null, "", "/?lang=sv");
    show(client(), memory(VIEWER));
    expect(await screen.findByLabelText("Din roll: Läsare")).toBeInTheDocument();
  });

  it("reads a server that cannot say who is signed in as nobody", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    const server = memory(VIEWER);
    server.me = vi.fn(async () => {
      throw new Error("offline");
    });
    show(client(), server);
    expect(await screen.findByLabelText("Your role: Not signed in")).toBeInTheDocument();
    expect(await screen.findByText("Sign in to keep origins.")).toBeInTheDocument();
  });

  it("keeps the point it starts from, with the areas worked out in the browser", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    const server = memory(VIEWER);
    show(client(HISTORY, NETWORK_ROWS), server);
    const user = userEvent.setup();
    const reached = await screen.findByRole("list", { name: "Stops within 30 minutes" });
    await user.click(within(reached).getAllByRole("button", { name: /: start from here$/ })[1]);
    expect(await screen.findByRole("textbox", { name: "Name" })).toHaveValue("Kaisaniemi");
    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(await screen.findByText("Kept: Kaisaniemi. Compare it on the Origins page.")).toBeInTheDocument();
    const kept = vi.mocked(server.keep).mock.calls[0][0];
    expect(kept.source).toBe("network");
    expect(kept.bands.map((band) => band.minutes)).toEqual([10, 20, 30]);
    await sweep(screen.getByRole("region", { name: "Reach" }));
    // The last stop's details stay open in the entity panel: its controls too.
    const panel = await screen.findByRole("dialog");
    await user.click(within(panel).getByRole("link", { name: "Open in the Portal" }));
    await sweep(panel);
  });
});
