import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider, ProblemError } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { AnalyserContext } from "./analysis";
import { HISTORY, KPIS } from "./fixtures/kpis";
import { inProcess } from "./test-analyser";
import { server } from "./test-server";

// The App's server, in memory (T-3350).
vi.mock("./server", async (original) => {
  const { server } = await import("./test-server");
  return { ...(await original<typeof import("./server")>()), kpiApi: () => server };
});
// jsdom has no canvas: the chart's option is tested on its own.
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const READ = {
  permissions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const }],
  prohibitions: [],
};
const PREFIX = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:";

function client(fixture: Partial<Parameters<typeof stubClient>[0]> = {}) {
  return stubClient({ entities: KPIS, temporal: HISTORY, access: READ, ...fixture }, { appName: "kpi-forecast" });
}

function show(c = client()) {
  render(
    <JcProvider client={c}>
      <AnalyserContext.Provider value={inProcess}>
        <App />
      </AnalyserContext.Provider>
    </JcProvider>,
  );
  return c;
}

const FI_SUMMARY =
  "4 mittaria, viimeiset 30 päivää: 1 nousee, 1 laskee, 1 pysyy ennallaan. Poikkeavia pisteitä: 1. 1 mittarilla on liian vähän historiaa ennusteeseen.";

describe("kpi-forecast", () => {
  beforeEach(() => {
    server.reset();
    window.history.replaceState(null, "", "/?lang=fi");
  });
  afterEach(() => window.history.replaceState(null, "", "/"));

  // AP-04: read through the app's own endpoint only, never written to; the first screen answers
  // how the indicators move without a click.
  it("answers how every indicator moves on arrival, in Finnish, read only through the app's endpoint", async () => {
    const c = show();
    expect(screen.getByRole("heading", { level: 1, name: "Helsingin mittarit: trendi ja ennuste" })).toBeInTheDocument();
    expect(await screen.findByText(FI_SUMMARY)).toBeInTheDocument();
    const list = screen.getByRole("list", { name: "Mittarit" });
    const buttons = within(list).getAllByRole("button");
    expect(buttons.map((b) => b.querySelector("strong")?.textContent)).toEqual([
      "Bikes available in the city bike network",
      "Demo counter",
      "Docking stations in the Helsinki city bike network",
      "Virtual stations in the city bike network",
    ]);
    // The first is shown in full, with its one odd reading.
    expect(buttons[0]).toHaveAttribute("aria-pressed", "true");
    expect(within(buttons[0]).getByText("1 poikkeavaa")).toBeInTheDocument();
    expect(within(buttons[1]).getByText(/^laskee, [−-]/)).toBeInTheDocument();
    expect(within(buttons[3]).getByText("Ei historiaa viimeisiltä 30 päivältä.")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Poikkeavat pisteet" })).toHaveTextContent(/3\.10\.2030 klo 15\.00: 1\s200, odotettu 2\s99\d/);
    expect(c.transport.calls.every((call) => call.method === "GET" && call.path.includes("/api/endpoint/"))).toBe(true);
    const temporal = c.transport.calls.find((call) => call.path.includes("/temporal/entities"));
    expect(temporal?.path).toContain("attrs=currentValue");
    expect(temporal?.path).toContain("timerel=after");
  });

  // T-3350: the day's forecasts recorded on the server, and the chosen one's earlier ones shown.
  it("has the server record the day's forecasts for the period, and lists the chosen indicator's earlier ones", async () => {
    show();
    expect(await screen.findByText(FI_SUMMARY)).toBeInTheDocument();
    expect(server.record).toHaveBeenCalledWith(30);
    expect(await screen.findByText(/^Tämän mittarin aiempia ennusteita ei ole vielä erääntynyt\./)).toBeInTheDocument();
    expect(server.list).toHaveBeenCalledWith(`${PREFIX}bikes-available-sum`);
  });

  it("shows the indicator a person picks, and the address carries it", async () => {
    show();
    await screen.findByText(FI_SUMMARY);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /^Demo counter/ }));
    expect(screen.getByRole("button", { name: /^Demo counter/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("figure")).toHaveTextContent("Demo counter: historia ja ennuste");
    expect(new URLSearchParams(window.location.search).get("kpi")).toBe(`${PREFIX}demo-counter-1`);
  });

  it("keeps only the indicators with odd points, and clears back to all", async () => {
    show();
    await screen.findByText(FI_SUMMARY);
    const user = userEvent.setup();
    await user.click(screen.getByRole("checkbox", { name: "Vain mittarit, joissa on poikkeamia" }));
    expect(within(screen.getByRole("list", { name: "Mittarit" })).getAllByRole("button")).toHaveLength(1);
    expect(window.location.search).toContain("odd=1");
    await user.click(screen.getByRole("button", { name: "Tyhjennä valinnat" }));
    expect(within(screen.getByRole("list", { name: "Mittarit" })).getAllByRole("button")).toHaveLength(4);
    expect(window.location.search).toBe("?lang=fi");
  });

  it("opens on the view its address names", async () => {
    window.history.replaceState(null, "", `/?lang=en&kpi=${encodeURIComponent(`${PREFIX}bike-network-stations`)}&days=90`);
    show();
    expect(await screen.findByText(/^4 indicators over the last 90 days: 1 rising, 1 falling, 1 flat\./)).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Period" })).toHaveValue("90");
    expect(screen.getByRole("button", { name: /^Docking stations/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("switches to English and keeps the language in the address", async () => {
    show();
    await screen.findByText(FI_SUMMARY);
    await userEvent.setup().selectOptions(screen.getByRole("combobox", { name: "Kieli" }), "en");
    expect(screen.getByRole("heading", { level: 1, name: "Helsinki KPIs: trend and forecast" })).toBeInTheDocument();
    expect(await screen.findByText(/^4 indicators over the last 30 days/)).toBeInTheDocument();
    expect(document.documentElement.lang).toBe("en");
  });

  it("says when there are no indicators at all", async () => {
    show(client({ entities: [], temporal: [] }));
    expect(await screen.findByText("Ei mittareita.")).toBeInTheDocument();
  });

  it("says the indicators failed, with the status, and reads again on Retry", async () => {
    const c = client();
    const all = c.entities.all.bind(c.entities);
    let failing = true;
    c.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      if (failing) throw new ProblemError(503, { title: "Service Unavailable", status: 503 });
      return all(type, query);
    }) as typeof c.entities.all;
    show(c);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Mittareita ei voitu lukea (HTTP 503). Yritä myöhemmin uudelleen.");
    failing = false;
    await userEvent.setup().click(within(alert).getByRole("button", { name: "Yritä uudelleen" }));
    expect(await screen.findByText(FI_SUMMARY)).toBeInTheDocument();
  });

  it("keeps the current values when the history is refused, and says so", async () => {
    const c = client();
    c.temporal.list = async () => {
      throw new ProblemError(403, { title: "Forbidden", status: 403 });
    };
    show(c);
    expect(await screen.findByRole("alert")).toHaveTextContent("Mittareiden historiaa ei voitu lukea (HTTP 403)");
    expect(await screen.findByText(/^4 mittaria, viimeiset 30 päivää: 0 nousee, 0 laskee, 0 pysyy ennallaan\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Bikes available/ })).toHaveTextContent(/Nyt 3\s848/);
  });

  // SDK-40: each indicator is chosen from the list, and the chosen one opens in the SDK's entity
  // panel; a public App writes nothing, so the panel offers no Edit.
  it("chooses every indicator from the list and opens the chosen one in the entity panel", async () => {
    show();
    await screen.findByText(FI_SUMMARY);
    const user = userEvent.setup();
    for (const name of ["Docking stations in the Helsinki city bike network", "Virtual stations in the city bike network", "Demo counter", "Bikes available in the city bike network"]) {
      await user.click(screen.getByRole("button", { name }));
      expect(screen.getByRole("button", { name })).toHaveAttribute("aria-pressed", "true");
    }
    expect(screen.getByRole("button", { name: "Bikes available in the city bike network" })).toHaveAccessibleDescription(/^Nyt 3\s848/);
    await user.selectOptions(screen.getByRole("combobox", { name: "Ajanjakso" }), "7");
    expect(new URLSearchParams(window.location.search).get("days")).toBe("7");
    await user.click(screen.getByRole("button", { name: "Kaikki tiedot" }));
    const panel = await screen.findByRole("dialog", { name: "Bikes available in the city bike network" });
    expect(within(panel).queryByRole("button", { name: "Muokkaa" })).toBeNull();
    await user.click(within(panel).getByRole("button", { name: "Sulje" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("in English: the period, the odd ones only, clearing, the panel, and back to Finnish", async () => {
    window.history.replaceState(null, "", "/?lang=en&days=90");
    show();
    await screen.findByText(/^4 indicators over the last 90 days/);
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole("combobox", { name: "Period" }), "30");
    await user.click(screen.getByRole("checkbox", { name: "Only indicators with points that look wrong" }));
    expect(within(screen.getByRole("list", { name: "Indicators" })).getAllByRole("button")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(window.location.search).toBe("?lang=en");
    await user.click(screen.getByRole("button", { name: "All details" }));
    const panel = await screen.findByRole("dialog");
    await user.click(within(panel).getByRole("button", { name: "Close" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Language" }), "fi");
    expect(screen.getByRole("heading", { level: 1, name: "Helsingin mittarit: trendi ja ennuste" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("lang")).toBe("fi");
  });

  it("says it is working while the history is read, and a history read that throws no Problem still says why", async () => {
    const c = client();
    // One read for the whole case, made before the page asks: under load the page's effect that
    // starts the read can run after "Lasketaan…" is found, and a reject captured only then was
    // still the no-op, so the case waited out the timeout (T-5329).
    let fail: (reason: unknown) => void = () => undefined;
    const read = new Promise<never>((_, reject) => (fail = reject));
    read.catch(() => undefined);
    c.temporal.list = () => read;
    show(c);
    expect(await screen.findByText("Lasketaan…")).toBeInTheDocument();
    // No answer from an empty history while the real one is read.
    expect(screen.queryByText(/^4 mittaria/)).toBeNull();
    await act(async () => fail(new TypeError("Failed to fetch")));
    expect(await screen.findByRole("alert")).toHaveTextContent("Mittareiden historiaa ei voitu lukea (HTTP 0)");
  });
});
