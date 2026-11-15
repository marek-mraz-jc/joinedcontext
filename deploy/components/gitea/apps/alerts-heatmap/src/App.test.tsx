import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider, ProblemError } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { AnalyserContext } from "./analysis";
import { ALERTS } from "./fixtures/alerts";
import { inProcess } from "./test-analyser";
import { ServerContext } from "./server";
import type { Server } from "./server";
import { fakeServer } from "./test-server";
import { Map as FakeMap, Popup } from "./testing/maplibre";

vi.mock("maplibre-gl", () => import("./testing/maplibre"));
// jsdom has no canvas: the chart's option is tested on its own.
const chart = vi.hoisted(() => ({ click: null as ((params: unknown) => void) | null }));
vi.mock("echarts", () => ({
  init: () => ({
    setOption: () => undefined,
    resize: () => undefined,
    dispose: () => undefined,
    on: (_event: string, handler: (params: unknown) => void) => {
      chart.click = handler;
    },
  }),
}));

const READ = {
  permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

const PORTAL = "https://portal.dev.joinedcontext.com/projects/helsinki";

function show(
  client = stubClient({ entities: ALERTS, access: READ }, { appName: "alerts-heatmap", portal: PORTAL, space: "helsinki" }),
  server: Server = fakeServer(),
) {
  render(
    <JcProvider client={client}>
      <ServerContext.Provider value={server}>
        <AnalyserContext.Provider value={inProcess}>
          <App />
        </AnalyserContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
  return client;
}

describe("alerts-heatmap", () => {
  beforeEach(() => window.history.replaceState(null, "", "/?lang=fi"));
  afterEach(() => {
    window.history.replaceState(null, "", "/");
    FakeMap.built.length = 0;
    FakeMap.snapshotFails = false;
    Popup.opened.length = 0;
  });

  // AP-04: read through the app's own endpoint only, never written to; the first screen answers
  // where and when without a click.
  it("answers where and when on arrival, in Finnish, read only through the app's endpoint", async () => {
    const client = show();
    expect(screen.getByRole("heading", { level: 1, name: "Missä ja milloin häiriöitä on" })).toBeInTheDocument();
    expect(
      await screen.findByText(
        "8 tiedotetta 7.10.2030–21.10.2030. Eniten kuusikulmiossa: 4 tiedotetta. Vilkkain alkamistunti: maanantai klo 8–9.",
      ),
    ).toBeInTheDocument();
    const places = screen.getByRole("list", { name: "Toistuvat paikat" });
    expect(within(places).getByText("Mannerheimintie, Helsinki. Tietyö.")).toBeInTheDocument();
    expect(within(places).getByText(/^4 tiedotetta, \d+ m säteellä$/)).toBeInTheDocument();
    expect(screen.getByRole("application", { name: "Kartta: tiedotteet kuusikulmioittain" })).toBeInTheDocument();
    expect(client.transport.calls.every((call) => call.method === "GET" && call.path.includes("/api/endpoint/"))).toBe(true);
    expect(client.transport.calls[0].path).toContain("attrs=name%2CsubCategory%2Caddress%2CdateIssued%2CvalidFrom%2Clocation");
  });

  it("filters by kind over the whole set, and the address carries the choice", async () => {
    show();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("checkbox", { name: "Onnettomuus (3)" }));
    expect(await screen.findByText(/^3 tiedotetta /)).toBeInTheDocument();
    expect(window.location.search).toContain("kind=carAccident");
    // The other kind stays offered, with its count.
    expect(screen.getByRole("checkbox", { name: "Tietyö (5)" })).not.toBeChecked();
    // Both kinds chosen is every alert again; the filter is still in the address.
    await user.click(screen.getByRole("checkbox", { name: "Tietyö (5)" }));
    expect(await screen.findByText(/^8 tiedotetta /)).toBeInTheDocument();
    expect(window.location.search).toContain("roadWorks");
    await user.click(screen.getByRole("button", { name: "Tyhjennä valinnat" }));
    expect(await screen.findByText(/^8 tiedotetta /)).toBeInTheDocument();
    expect(window.location.search).toBe("?lang=fi");
  });

  it("narrows to a period, both ends checked against each other, and back", async () => {
    show();
    const user = userEvent.setup();
    await screen.findByText(/^8 tiedotetta /);
    const from = screen.getByLabelText("Alkaen");
    const to = screen.getByLabelText("Päättyen");
    // Enter in a field submits nothing: the page does not reload.
    await user.type(from, "{Enter}");
    expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();
    await user.type(from, "2030-10-13");
    await user.type(to, "2030-10-20");
    expect(to).toHaveAttribute("min", "2030-10-13");
    expect(from).toHaveAttribute("max", "2030-10-20");
    expect(await screen.findByText(/^1 tiedotetta |^1 tiedote /)).toBeInTheDocument();
    await user.clear(from);
    await user.clear(to);
    expect(await screen.findByText(/^8 tiedotetta /)).toBeInTheDocument();
  });

  it("opens a repeat place's alert in the panel from the list and from the map, with a Portal link and no Edit", async () => {
    show();
    const user = userEvent.setup();
    await screen.findByText(/^8 tiedotetta /);
    await user.click(screen.getByRole("button", { name: "Tiedot: Mannerheimintie, Helsinki. Tietyö." }));
    const panel = await screen.findByRole("dialog", { name: "Mannerheimintie, Helsinki. Tietyö." });
    const link = await within(panel).findByRole("link", { name: "Avaa portaalissa" });
    expect(link).toHaveAttribute("href", `${PORTAL}/explore?space=helsinki&entityId=${encodeURIComponent(ALERTS[0].id)}`);
    expect(within(panel).queryByRole("button", { name: "Muokkaa" })).toBeNull();
    // Following it leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    await user.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    await user.click(within(panel).getByRole("button", { name: "Sulje" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    const map = await waitFor(() => {
      const [built] = FakeMap.built;
      expect((built?.sources.places?.data as { features: unknown[] } | undefined)?.features.length).toBeGreaterThan(0);
      return built!;
    });
    map.fire("click", { features: [{ properties: { index: 0 } }], lngLat: { lng: 24.93, lat: 60.17 } }, "places");
    expect(await screen.findByRole("dialog", { name: "Mannerheimintie, Helsinki. Tietyö." })).toBeInTheDocument();
    expect(Popup.opened[0].content?.textContent).toContain("Mannerheimintie");
  });

  it("opens on the view its address names", async () => {
    window.history.replaceState(null, "", "/?lang=en&kind=roadWorks&day=0&hour=8");
    show();
    expect(await screen.findByText(/^4 alerts from 7 Oct 2030 to 21 Oct 2030\./)).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: "Show every hour, not only Monday 08:00–09:00" }));
    expect(screen.queryByRole("button", { name: /^Show every hour/ })).toBeNull();
    expect(window.location.search).not.toContain("hour=");
  });

  it("switches to English and keeps the language in the address", async () => {
    show();
    await screen.findByText(/^8 tiedotetta/);
    await userEvent.setup().selectOptions(screen.getByRole("combobox", { name: "Kieli" }), "en");
    expect(screen.getByRole("heading", { level: 1, name: "Where and when alerts happen" })).toBeInTheDocument();
    expect(await screen.findByText(/^8 alerts from 7 Oct 2030 to 21 Oct 2030\./)).toBeInTheDocument();
    expect(document.documentElement.lang).toBe("en");
    expect(new URLSearchParams(window.location.search).get("lang")).toBe("en");
    // The English page's own controls, each tried once.
    const user = userEvent.setup();
    await user.click(screen.getByRole("checkbox", { name: "Road work (5)" }));
    expect(await screen.findByText(/^5 alerts /)).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: "Accident (3)" }));
    await user.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(await screen.findByText(/^8 alerts /)).toBeInTheDocument();
    await user.type(screen.getByLabelText("From"), "2030-10-01");
    await user.type(screen.getByLabelText("To"), "2030-10-31");
    await user.click(screen.getByRole("button", { name: "Details of Mannerheimintie, Helsinki. Tietyö." }));
    const panel = await screen.findByRole("dialog", { name: "Mannerheimintie, Helsinki. Tietyö." });
    const link = await within(panel).findByRole("link", { name: "Open in the Portal" });
    link.addEventListener("click", (event) => event.preventDefault());
    await user.click(link);
    await user.click(within(panel).getByRole("button", { name: "Close" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Language" }), "fi");
    expect(screen.getByRole("heading", { level: 1, name: "Missä ja milloin häiriöitä on" })).toBeInTheDocument();
  });

  it("picks an hour of the week from its cell, and the same cell again lets it go", async () => {
    show();
    await screen.findByText(/^8 tiedotetta /);
    await waitFor(() => expect(chart.click).not.toBeNull());
    // Row 6 is Monday, column 8 its 08:00 hour.
    act(() => chart.click?.({ value: [8, 6, 4] }));
    expect(new URLSearchParams(window.location.search).get("hour")).toBe("8");
    const unpick = await screen.findByRole("button", { name: /^Näytä kaikki tunnit/ });
    await userEvent.setup().click(unpick);
    expect(new URLSearchParams(window.location.search).get("hour")).toBeNull();
    act(() => chart.click?.({ value: [8, 6, 4] }));
    act(() => chart.click?.({ name: "a legend, not a cell" }));
    expect(new URLSearchParams(window.location.search).get("hour")).toBe("8");
    act(() => chart.click?.({ value: [8, 6, 4] }));
    expect(new URLSearchParams(window.location.search).get("hour")).toBeNull();
  });

  it("says when there are no alerts at all", async () => {
    show(stubClient({ entities: [], access: READ }, { appName: "alerts-heatmap" }));
    expect(await screen.findByText("Ei tiedotteita.")).toBeInTheDocument();
  });

  it("says the endpoint failed, with its status, and reads again on Retry", async () => {
    const client = stubClient({ entities: ALERTS, access: READ }, { appName: "alerts-heatmap" });
    const all = client.entities.all.bind(client.entities);
    let failing = true;
    client.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      if (failing) throw new ProblemError(503, { title: "Service Unavailable", status: 503 });
      return all(type, query);
    }) as typeof client.entities.all;
    show(client);
    const alert = await screen.findByText("Tiedotteita ei voitu lukea (HTTP 503). Yritä myöhemmin uudelleen.").then((text) => text.closest("[role=alert]") as HTMLElement);
    failing = false;
    await userEvent.setup().click(within(alert).getByRole("button", { name: "Yritä uudelleen" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(await screen.findByText(/^8 tiedotetta /)).toBeInTheDocument();
  });

  // T-3351: the view saved on the App's server with its places and a picture of the map, and
  // shown again from the list, the address back as it was saved.
  it("saves the view as a report with its map picture, and shows it again from the list", async () => {
    window.history.replaceState(null, "", "/?lang=fi&kind=roadWorks");
    const server = fakeServer();
    show(undefined, server);
    const user = userEvent.setup();
    await screen.findByText(/tiedotetta /);
    expect(await screen.findByText("Ei vielä raportteja.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Tallenna näkymä" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Anna raportille nimi.");
    await user.type(screen.getByLabelText("Raportin nimi"), "  Tietyöt  ");
    await user.click(screen.getByRole("button", { name: "Tallenna näkymä" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Raportti tallennettu: Tietyöt.");
    expect(server.saved[0]).toMatchObject({ title: "Tietyöt", view: "lang=fi&kind=roadWorks" });
    expect(server.saved[0]?.places.length).toBeGreaterThan(0);
    expect(server.pictures.get(1)?.type).toBe("image/png");
    expect(screen.getByLabelText("Raportin nimi")).toHaveValue("");

    // Another view, then the saved one again.
    await user.click(screen.getByRole("button", { name: "Tyhjennä valinnat" }));
    await waitFor(() => expect(window.location.search).not.toContain("kind="));
    await user.click(screen.getByRole("button", { name: "Näytä raportti Tietyöt" }));
    await waitFor(() => expect(window.location.search).toContain("kind=roadWorks"));
    expect(screen.getByRole("status")).toHaveTextContent("Näytetään raportti Tietyöt.");

    const open = vi.spyOn(window, "open").mockReturnValue(null);
    await user.click(screen.getByRole("button", { name: "Avaa raportin Tietyöt karttakuva" }));
    await waitFor(() => expect(open).toHaveBeenCalledWith("https://store.test/apps/0/x/reports/1/map.png", "_blank", "noopener"));
    open.mockRestore();
  });

  it("saves a report without a picture when the map cannot take one, and says so", async () => {
    const server = fakeServer();
    FakeMap.snapshotFails = true;
    show(undefined, server);
    const user = userEvent.setup();
    await screen.findByText(/tiedotetta /);
    await user.type(screen.getByLabelText("Raportin nimi"), "Ilman kuvaa");
    await user.click(screen.getByRole("button", { name: "Tallenna näkymä" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Raportti tallennettu: Ilman kuvaa. Karttakuvaa ei voitu ottaa.");
    expect(server.pictures.size).toBe(0);
    expect(screen.queryByRole("button", { name: "Avaa raportin Ilman kuvaa karttakuva" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Näytä raportti Ilman kuvaa" }));
    expect(screen.getByRole("status")).toHaveTextContent("Näytetään raportti Ilman kuvaa.");
  });

  it("says why a report was not saved, and why its picture did not open", async () => {
    show(undefined, fakeServer(undefined, { save: "a report's title has 1 to 120 characters" }));
    const user = userEvent.setup();
    await screen.findByText(/tiedotetta /);
    await user.type(screen.getByLabelText("Raportin nimi"), "x");
    await user.click(screen.getByRole("button", { name: "Tallenna näkymä" }));
    expect(await screen.findByText("Raporttia ei voitu tallentaa: a report's title has 1 to 120 characters")).toBeInTheDocument();
  });

  it("keeps a saved report when its picture cannot be stored, and says a missing picture in words", async () => {
    const server = fakeServer(undefined, { attach: "Forbidden", picture: "this report has no map snapshot" });
    server.saved.push({ id: 7, title: "Vanha", view: "lang=fi", kept: 2, places: [], has_snapshot: true, created_at: "2030-10-01T08:00:00Z" });
    show(undefined, server);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Avaa raportin Vanha karttakuva" }));
    expect(await screen.findByText("Karttakuvaa ei voitu avata: this report has no map snapshot")).toBeInTheDocument();
    await screen.findByText(/tiedotetta /);
    await user.type(screen.getByLabelText("Raportin nimi"), "Uusi");
    await user.click(screen.getByRole("button", { name: "Tallenna näkymä" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Raportti tallennettu: Uusi. Karttakuvaa ei voitu ottaa.");
    await user.click(screen.getByRole("button", { name: "Näytä raportti Vanha" }));
    expect(screen.getByRole("status")).toHaveTextContent("Näytetään raportti Vanha.");
    await user.click(screen.getByRole("button", { name: "Näytä raportti Uusi" }));
    expect(screen.getByRole("status")).toHaveTextContent("Näytetään raportti Uusi.");
  });

  it("saves a report in English", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    const server = fakeServer();
    show(undefined, server);
    const user = userEvent.setup();
    await screen.findByText(/ alerts from /);
    await user.type(screen.getByLabelText("Report name"), "Mondays");
    await user.click(screen.getByRole("button", { name: "Save this view" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Report saved: Mondays.");
    expect(server.saved[0]?.view).toBe("lang=en");
    await user.click(screen.getByRole("button", { name: "Show the report Mondays" }));
    expect(screen.getByRole("status")).toHaveTextContent("Showing the report Mondays.");
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    await user.click(screen.getByRole("button", { name: "Open the map picture of Mondays" }));
    await waitFor(() => expect(open).toHaveBeenCalledWith("https://store.test/apps/0/x/reports/1/map.png", "_blank", "noopener"));
    open.mockRestore();
  });

  it("lists the weeks the server keeps, the newest first, and says when they are as last kept", async () => {
    const weeks = {
      weeks: [
        { week: "2030-10-21", alerts: 4, places: [], computed_at: "2030-10-21T10:00:00Z" },
        { week: "2030-10-14", alerts: 12, places: [{ name: "Hämeentie 3", lon: 24.95, lat: 60.17, count: 5 }, { name: "", lon: 25, lat: 60.2, count: 3 }], computed_at: "2030-10-21T10:00:00Z" },
        { week: "2030-10-07", alerts: 3, places: [{ name: "", lon: 24.9512, lat: 60.1701, count: 3 }], computed_at: "2030-10-21T10:00:00Z" },
      ],
      stale: true,
    };
    show(undefined, fakeServer(weeks));
    const table = await screen.findByRole("table", { name: "Toistuvat paikat viikoittain" });
    const rows = within(table).getAllByRole("row");
    expect(rows.map((row) => row.textContent)).toEqual([
      "Viikko alkaenTiedotteitaToistuvia paikkojaEniten tiedotteita",
      "21.10.203040–",
      "14.10.2030122Hämeentie 3 (5)",
      "7.10.20303160.1701, 24.9512 (3)",
    ]);
    expect(screen.getByText("Syötettä ei voitu lukea juuri nyt: viikot ovat viimeksi tallennetut.")).toBeInTheDocument();
  });

  it("says when the server keeps no week yet, and when the weeks or reports cannot be read", async () => {
    show(undefined, fakeServer(undefined, { weeks: "the gateway could not answer right now; try again shortly (502)", reports: "the database is not reachable" }));
    expect(await screen.findByText("Viikkoja ei voitu lukea: the gateway could not answer right now; try again shortly (502)")).toBeInTheDocument();
    expect(await screen.findByText("Raportteja ei voitu lukea: the database is not reachable")).toBeInTheDocument();
  });

  it("says when the server has kept no week yet", async () => {
    show();
    expect(await screen.findByText("Palvelin ei ole vielä tallentanut yhtään viikkoa.")).toBeInTheDocument();
  });
});
