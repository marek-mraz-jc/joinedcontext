/**
 * T-3827: the roles. A steward publishes an indicator's forecast with the period on screen, a
 * horizon and a note, which every reader sees, and withdraws one; a signed-in reader follows
 * indicators of their own; the history shows a viewer their own changes and a steward everyone's.
 * Signed out, every control that needs a sign-in stays in sight, closed, with the reason.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { AnalyserContext } from "./analysis";
import { horizonOf } from "./components/Published";
import { HISTORY, KPIS } from "./fixtures/kpis";
import { openHref } from "./pages/Follows";
import { ServerProblem } from "./problem";
import type { PublishedForecast } from "./server";
import { inProcess } from "./test-analyser";
import { memoryServer, STEWARD, VIEWER } from "./test-server";
import { sweep } from "./testing/sweep";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const READ = {
  permissions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const }],
  prohibitions: [],
};
const PREFIX = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:";
const FIRST = `${PREFIX}bikes-available-sum`;
const NAME = "Bikes available in the city bike network";
const DAY = 86_400_000;
const server = memoryServer();

function show(language = "en") {
  window.history.replaceState(null, "", `/?lang=${language}`);
  const c = stubClient({ entities: KPIS, temporal: HISTORY, access: READ }, { appName: "kpi-forecast" });
  return render(
    <JcProvider client={c}>
      <AnalyserContext.Provider value={inProcess}>
        <App server={server} />
      </AnalyserContext.Provider>
    </JcProvider>,
  );
}
const open = async (page: string) => fireEvent.click(await screen.findByRole("button", { name: page }));
const statuses = () => screen.getAllByRole("status").map((s) => s.textContent);
const publication = (id: number, note: string, at: string): PublishedForecast => ({
  id,
  days: 30,
  horizon: 2,
  note,
  step: DAY,
  points: [
    { t: Date.UTC(2030, 9, 21, 9), v: 120, lo: 110, hi: 130 },
    { t: Date.UTC(2030, 9, 22, 9), v: 121, lo: 109, hi: 133 },
  ],
  publisherName: "Sami Steward",
  publishedAt: at,
});

beforeEach(() => server.reset());
afterEach(() => window.history.replaceState(null, "", "/"));

describe("the parts", () => {
  it("takes a horizon of 1 to 48 whole steps, and opens a followed indicator by its address", () => {
    expect(["1", " 12 ", "48"].map(horizonOf)).toEqual([1, 12, 48]);
    expect(["0", "49", "", "2.5", "-1", "x"].map(horizonOf)).toEqual([null, null, null, null, null, null]);
    expect(openHref(FIRST, "sv")).toBe(`?kpi=${encodeURIComponent(FIRST)}&lang=sv#kpis`);
  });
});

describe("signed out", () => {
  it("sees the published forecast, and every control that needs a sign-in closed with its reason", async () => {
    server.publications.set(FIRST, [publication(1, "For the board", "2030-10-20T06:00:00Z")]);
    show();
    expect(await screen.findByLabelText("Your role: Not signed in")).toBeInTheDocument();
    expect(await screen.findByText("For the board")).toBeInTheDocument();
    expect(screen.getByText(/^Sami Steward published on 20 Oct 2030, 09:00: from 30 days of history, 2 steps ahead\.$/)).toBeInTheDocument();
    expect(screen.getByText("120 (110–130)")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Withdraw/ })).toBeNull();
    const follow = screen.getByRole("button", { name: "Follow" });
    expect(follow).toBeDisabled();
    expect(follow).toHaveAttribute("title", "Sign in to follow indicators.");
    const form = screen.getByRole("form", { name: "Publish the forecast" });
    expect(within(form).getByRole("button", { name: "Publish the forecast" })).toBeDisabled();
    expect(within(form).getByRole("button", { name: "Publish the forecast" })).toHaveAttribute("title", "Only a steward publishes forecasts.");
    expect(within(form).getByRole("textbox", { name: "Note" })).toBeDisabled();
    await open("Followed");
    expect(await screen.findByText("Sign in to follow indicators.")).toBeInTheDocument();
    await open("History");
    expect(await screen.findByText("Sign in to see the history.")).toBeInTheDocument();
    expect(server.follows).not.toHaveBeenCalled();
    expect(server.history).not.toHaveBeenCalled();
  });

  it("is what the page assumes when the server cannot say who calls", async () => {
    vi.mocked(server.me).mockRejectedValueOnce(new ServerProblem(502, ""));
    show();
    expect(await screen.findByLabelText("Your role: Not signed in")).toBeInTheDocument();
  });
});

describe("a viewer", () => {
  beforeEach(() => {
    server.who = VIEWER;
  });

  it("follows the chosen indicator, finds it under Followed, lets it go there, and sees it in their history", async () => {
    show();
    expect(await screen.findByLabelText("Your role: Viewer")).toBeInTheDocument();
    expect(await screen.findByText("No forecast of this indicator is published.")).toBeInTheDocument();
    expect(screen.getByText("Only a steward publishes forecasts.")).toBeInTheDocument();
    const follow = await screen.findByRole("button", { name: "Follow" });
    await waitFor(() => expect(follow).toBeEnabled());
    fireEvent.click(follow);
    expect(await screen.findByRole("button", { name: "Stop following" })).toHaveAttribute("aria-pressed", "true");
    expect(statuses()).toContain(`You now follow ${NAME}.`);
    expect(server.follow).toHaveBeenCalledWith(FIRST);

    await open("Followed");
    const list = await screen.findByRole("list", { name: "Indicators you follow" });
    expect(within(list).getByText(NAME)).toBeInTheDocument();
    expect(within(list).getByRole("link", { name: `Open: ${NAME}` })).toHaveAttribute("href", openHref(FIRST, "en"));
    vi.mocked(server.unfollow).mockRejectedValueOnce(new ServerProblem(0, ""));
    fireEvent.click(within(list).getByRole("button", { name: `Stop following: ${NAME}` }));
    await waitFor(() => expect(statuses()).toContain("The server could not be reached. Check the connection and try again."));
    expect(within(screen.getByRole("list", { name: "Indicators you follow" })).getByText(NAME)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: `Stop following: ${NAME}` }));
    expect(await screen.findByText("You follow no indicator yet. Choose one and press Follow.")).toBeInTheDocument();
    expect(statuses()).toContain(`You no longer follow ${NAME}.`);

    await open("History");
    expect(await screen.findByText("Your own changes; a steward sees everyone's.")).toBeInTheDocument();
    const table = await screen.findByRole("table");
    expect(within(table).getByText(/added: followed an indicator/)).toBeInTheDocument();
    expect(within(table).getByText(/removed: stopped following an indicator/)).toBeInTheDocument();
  });

  it("is told when following is refused, and when their follows or the history cannot be read", async () => {
    server.followed.push({ id: 1, kpi: FIRST, owner: VIEWER.id });
    vi.mocked(server.unfollow).mockRejectedValueOnce(new ServerProblem(404, ""));
    show();
    const stop = await screen.findByRole("button", { name: "Stop following" });
    fireEvent.click(stop);
    await waitFor(() => expect(statuses()).toContain("It is no longer there, or it is not yours to change."));
    expect(screen.getByRole("button", { name: "Stop following" })).toBeInTheDocument();

    vi.mocked(server.follows).mockRejectedValueOnce(new ServerProblem(502, ""));
    vi.mocked(server.history).mockRejectedValueOnce(new ServerProblem(502, ""));
    vi.mocked(server.published).mockRejectedValueOnce(new ServerProblem(502, ""));
    show("en");
    await waitFor(() => expect(statuses().filter((s) => s === "The server did not answer. Try again in a moment.").length).toBeGreaterThan(0));
  });

  it("says so when the history cannot be read, and reads it again", async () => {
    vi.mocked(server.history).mockRejectedValueOnce(new ServerProblem(502, ""));
    show();
    await open("History");
    expect(await screen.findByText("The server did not answer. Try again in a moment.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("No change yet.")).toBeInTheDocument();
  });
});

describe("a steward", () => {
  beforeEach(() => {
    server.who = STEWARD;
  });

  it("publishes a forecast with a horizon and a note, sees it first, withdraws it, and reads everyone's history", async () => {
    server.publications.set(FIRST, [publication(1, "Last week's", "2030-10-13T06:00:00Z")]);
    server.log.push({ id: 1, at: "2030-10-19T06:00:00Z", actorName: "Vilja Viewer", action: "insert", target: "follows/9", summary: "followed an indicator", owner: VIEWER.id });
    show();
    expect(await screen.findByLabelText("Your role: Steward")).toBeInTheDocument();
    const form = await screen.findByRole("form", { name: "Publish the forecast" });
    expect(screen.getByText("A steward chooses the period and how far ahead; the server makes the forecast and every reader sees it.")).toBeInTheDocument();
    const publish = within(form).getByRole("button", { name: "Publish the forecast" });
    expect(publish).toBeDisabled();
    fireEvent.change(within(form).getByRole("textbox", { name: "Note" }), { target: { value: "  For the board  " } });
    fireEvent.change(within(form).getByRole("textbox", { name: "Steps ahead (1–48)" }), { target: { value: "49" } });
    expect(publish).toBeDisabled();
    fireEvent.change(within(form).getByRole("textbox", { name: "Steps ahead (1–48)" }), { target: { value: "6" } });
    expect(publish).toBeEnabled();
    expect(publish).not.toHaveAttribute("title");
    fireEvent.click(publish);
    await waitFor(() => expect(statuses()).toContain("The forecast is published."));
    expect(server.publish).toHaveBeenCalledWith(FIRST, 30, 6, "For the board");
    expect(screen.getByText("For the board")).toBeInTheDocument();
    expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(7);
    expect(within(screen.getByRole("list", { name: "Earlier publications" })).getByText(/Last week's/)).toBeInTheDocument();
    expect(within(form).getByRole("textbox", { name: "Note" })).toHaveValue("");

    // Withdrawn at once, back when the server refuses; then for good.
    const withdraws = screen.getAllByRole("button", { name: /^Withdraw: / });
    expect(withdraws).toHaveLength(2);
    fireEvent.click(withdraws[0]);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    vi.mocked(server.withdraw).mockRejectedValueOnce(new ServerProblem(403, ""));
    fireEvent.click(screen.getAllByRole("button", { name: /^Withdraw: / })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Withdraw for good" }));
    await waitFor(() => expect(statuses()).toContain("Your account may not do this."));
    expect(screen.getByText("For the board")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: /^Withdraw: / })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Withdraw for good" }));
    await waitFor(() => expect(statuses()).toContain("The forecast is withdrawn."));
    expect(screen.queryByText("For the board")).toBeNull();
    expect(screen.getByText("Last week's")).toBeInTheDocument();

    await open("History");
    expect(await screen.findByText("Everyone's changes, newest first.")).toBeInTheDocument();
    const table = await screen.findByRole("table");
    expect(within(table).getByText("Vilja Viewer")).toBeInTheDocument();
    expect(within(table).getByText(/removed: withdrew a published forecast/)).toBeInTheDocument();
  });

  it("is told in words when the server will not forecast", async () => {
    vi.mocked(server.publish).mockRejectedValueOnce(new ServerProblem(409, "this indicator has too little history over the period to forecast"));
    show();
    const form = await screen.findByRole("form", { name: "Publish the forecast" });
    fireEvent.change(within(form).getByRole("textbox", { name: "Note" }), { target: { value: "x" } });
    fireEvent.click(within(form).getByRole("button", { name: "Publish the forecast" }));
    await waitFor(() => expect(statuses()).toContain("The server refused: this indicator has too little history over the period to forecast"));
  });

  for (const [language, pages] of [
    ["en", ["Followed", "History", "Indicators"]],
    ["fi", ["Seuratut", "Historia", "Mittarit"]],
    ["sv", ["Följda", "Historik", "Mätare"]],
  ] as const) {
    it(`uses every control of every page once, in ${language}`, async () => {
      // Every indicator has publications, as the sweep chooses each in turn.
      KPIS.forEach((kpi, i) =>
        server.publications.set(kpi.id, [publication(10 * i + 1, "Last week's", "2030-10-13T06:00:00Z"), publication(10 * i + 2, "Before", "2030-10-06T06:00:00Z")]),
      );
      server.followed.push({ id: 1, kpi: FIRST, owner: STEWARD.id });
      const { container } = show(language);
      await waitFor(() => expect(server.published).toHaveBeenCalled());
      await screen.findByText("Last week's");
      // The page's own controls; the shell's pages are opened one by one.
      const main = () => container.querySelector<HTMLElement>("main") ?? container;
      await act(async () => sweep(main()));
      for (const page of pages) {
        await open(page);
        await act(async () => new Promise((done) => setTimeout(done, 0)));
        await act(async () => sweep(main()));
      }
      // Back on the indicators, once the analysis settled and the publications were read again.
      await screen.findByText("Last week's");
      await act(async () => sweep(main()));
    }, SWEEP);
  }
});

describe("in Swedish", () => {
  it("speaks Swedish in the header and on the pages", async () => {
    server.who = VIEWER;
    show("sv");
    expect(await screen.findByLabelText("Din roll: Läsare")).toBeInTheDocument();
    expect(await screen.findByText("Ingen prognos för den här mätaren är publicerad.")).toBeInTheDocument();
    await open("Följda");
    expect(await screen.findByText("Du följer ingen mätare ännu. Välj en och tryck på Följ.")).toBeInTheDocument();
  });
});
