/**
 * The events dashboard over what the public endpoint of `bbsk-podujatia` answers (T-4897).
 *
 * `fetch` is what is stubbed, so every case goes through the SDK's own endpoint source. MapLibre
 * needs WebGL, which jsdom does not have, so the library is a double recording what it was given.
 * Only `Date` is faked: the day is 2026-10-21 in every case.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { EntitySource } from "@joinedcontext/sdk";

const setData = vi.fn();
/** What the map does on a click of a drawn event, as MapLibre would call it. */
let clickEvent: ((event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) | null = null;

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) {
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && handler) clickEvent = handler;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { default: App, loadEvents, MonthView, MOST, Subscribe } = await import("./App");
const { answer, EVENTS } = await import("./fixtures/events");
const { eventOf, NO_FILTERS } = await import("./events");
const { LOCALES } = await import("./locales");
const s = LOCALES.sk;

const SLUG = "k2m4p6r8t0v2x4z6b8d0f2h4";
const ICS = `http://localhost:3000/api/endpoint/${SLUG}/ical/v1/events.ics`;

function show({ fail, rows = EVENTS, withEndpoint = true, endpoints, language = "sk" }: { fail?: unknown; rows?: unknown[]; withEndpoint?: boolean; endpoints?: { name: string; slug: string; space: string; types: string[] }[]; language?: string } = {}) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      if (fail instanceof Error) throw fail;
      if (fail) {
        return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the policy does not grant this type" }), { status: 403, headers: { "content-type": "application/problem+json" } });
      }
      return new Response(JSON.stringify(answer(new URL(path, "http://portal.test"), rows)), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
  // The list reads through `fetch`, the entity panel through the client: the same rows on both.
  const client = stubClient({ entities: (rows as Record<string, unknown>[]).map((entity) => projectRow(toRichRow(entity, "sk"), "sk")) }, {
    slug: withEndpoint ? SLUG : "",
    orgDomain: "bbsk.sk",
    space: withEndpoint ? "bbsk-podujatia" : "elsewhere",
    transport: "origin",
    appName: "bbsk-podujatia",
    language,
    ...(endpoints ? { endpoints } : {}),
    portal: "https://portal.bbsk.sk/projects/bbsk",
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-21T08:00:00Z"));
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  clickEvent = null;
});

/** The fixture's events as the dashboard reads them; the first is the exhibition of 22 October. */
const loadedEvents = () => EVENTS.map((entity) => eventOf(toRichRow(entity, "sk"), "sk")).filter((event) => event !== null);

const list = () => document.querySelector(".map-screen .events, .jc-tabpanel > .events") as HTMLElement | null;
const names = () => [...(list()?.querySelectorAll(".name") ?? [])].map((name) => name.textContent);
const loaded = () => waitFor(() => expect(names().length).toBeGreaterThan(0));
const tab = (name: string) => screen.getByRole("tab", { name });

/** Clicks or changes every usable control under `scope`; a link is followed nowhere, a panel opened is closed. */
function useEvery(scope: HTMLElement) {
  const controls = [...scope.querySelectorAll<HTMLElement>("button, a[href], input, select")];
  for (const control of controls) {
    if (!control.isConnected || (control as HTMLButtonElement).disabled) continue;
    if (control instanceof HTMLAnchorElement) {
      control.addEventListener("click", (event) => event.preventDefault(), { once: true });
      fireEvent.click(control);
    } else if (control instanceof HTMLInputElement || control instanceof HTMLSelectElement) {
      fireEvent.change(control, { target: { value: control.value } });
    } else {
      fireEvent.click(control);
    }
    const dialog = document.querySelector('[role="dialog"]');
    if (!dialog) continue;
    // The panel links the event to the Portal: followed nowhere, then the panel is closed.
    for (const link of within(dialog as HTMLElement).queryAllByRole("link")) {
      link.addEventListener("click", (event) => event.preventDefault(), { once: true });
      fireEvent.click(link);
    }
    fireEvent.click(within(dialog as HTMLElement).getAllByRole("button").at(-1) as HTMLElement);
  }
}

/** Picks every day of the month drawn and uses what the day lists. */
function everyDay() {
  for (const day of [...document.querySelectorAll<HTMLButtonElement>(".month-grid .day")]) {
    fireEvent.click(day);
    useEvery(document.querySelector(".day-events") as HTMLElement);
  }
}

describe("the events dashboard", () => {
  it("shows the events of fourteen towns on the map and in its list, soonest first", async () => {
    show();
    await loaded();
    expect(names()).toHaveLength(EVENTS.length - 1);
    expect(names()[0]).toBe("Výstava fotografií Pohronie");
    expect(screen.getByText(s.results(EVENTS.length - 1))).toHaveAttribute("role", "status");
    expect(screen.getByRole("combobox", { name: s.town }).querySelectorAll("option")).toHaveLength(16);
    await waitFor(() => expect((setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined)?.features).toHaveLength(EVENTS.length - 2));
    expect(screen.getByText(s.notOnMap(1))).toBeInTheDocument();
  });

  it("filters by town, category, dates and text, says when nothing matches, and clears", async () => {
    show();
    await loaded();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.town }), "Zvolen");
    expect(names()).toEqual(["Zvolenské divadelné dni", "Beseda o histórii zámku"]);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.category }), "Hudba");
    expect(screen.getByText(s.noResults)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: s.clear }));
    expect(names()).toHaveLength(EVENTS.length - 1);
    expect(screen.getByRole("button", { name: s.clear })).toBeDisabled();
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "jarmok");
    // The category is searched too: the Advent market is a "Jarmok".
    expect(names()).toEqual(["Jesenný jarmok", "Gemerský jarmok remesiel", "Žarnovický adventný trh"]);
    fireEvent.change(screen.getByLabelText(s.from), { target: { value: "2026-11-20" } });
    expect(names()).toEqual(["Žarnovický adventný trh"]);
    fireEvent.change(screen.getByLabelText(s.to), { target: { value: "2026-10-31" } });
    expect(screen.getByRole("alert")).toHaveTextContent(s.rangeInvalid);
    expect(screen.getByLabelText(s.from)).toHaveAttribute("aria-invalid", "true");
  });

  it("lists the organiser's page of an event as a link, and never a script URL", async () => {
    show();
    await loaded();
    await userEvent.click(tab(s.view.list));
    const links = screen.getAllByRole("link", { name: new RegExp(s.sourcePage) }).map((link) => link.getAttribute("href"));
    expect(links).toContain("https://www.banskabystrica.sk/podujatia/jesenny-jarmok/");
    expect(links.some((href) => href?.startsWith("javascript:"))).toBe(false);
    expect(links).toHaveLength(EVENTS.length - 3);
    for (const link of screen.getAllByRole("link", { name: new RegExp(s.sourcePage) })) expect(link).toHaveAttribute("rel", "noopener noreferrer");
  });

  it("opens an event from the list in the SDK's panel, and Escape gives the focus back", async () => {
    show();
    await loaded();
    const item = within(list() as HTMLElement).getByRole("button", { name: /Beseda o histórii zámku/ });
    await userEvent.click(item);
    expect(item).toHaveAttribute("aria-pressed", "true");
    const panel = await screen.findByRole("dialog", { name: "Beseda o histórii zámku" });
    expect(await within(panel).findByText("Zvolenský zámok")).toBeInTheDocument();
    // A public App links the event to the Portal and offers no Edit of its own (AP-140).
    const portal = within(panel).getByRole("link", { name: "Otvoriť v Portáli" });
    portal.addEventListener("click", (event) => event.preventDefault(), { once: true });
    fireEvent.click(portal);
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(item).toHaveFocus());
  });

  it("opens an event clicked on the map, through the endpoint the Portal lists, and ignores a click on nothing", async () => {
    show({ withEndpoint: false, endpoints: [{ name: "app-bbsk-podujatia", slug: SLUG, space: "bbsk-podujatia", types: ["Event"] }] });
    await loaded();
    await waitFor(() => expect(clickEvent).not.toBeNull());
    act(() => clickEvent?.({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => clickEvent?.({ features: [{ properties: { id: EVENTS[1].id } }] }));
    expect(await screen.findByRole("dialog", { name: "Koncert Štátnej opery" })).toBeInTheDocument();
  });

  it("draws the month with the events on their days, lists a picked day and moves between months", async () => {
    show();
    await loaded();
    await userEvent.click(tab(s.view.calendar));
    expect(screen.getByRole("heading", { level: 2, name: /október 2026/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: s.thisMonth })).toBeDisabled();
    const day = screen.getByRole("button", { name: /30\. októbra: 2 podujatia/ });
    expect(screen.getByText(s.pickDay)).toBeInTheDocument();
    await userEvent.click(day);
    expect(day).toHaveAttribute("aria-pressed", "true");
    const section = screen.getByRole("region", { name: /30\. októbra: 2 podujatia/ });
    expect([...section.querySelectorAll(".name")].map((name) => name.textContent)).toEqual(["Výstava fotografií Pohronie", "Zvolenské divadelné dni"]);
    await userEvent.click(screen.getByRole("button", { name: s.nextMonth }));
    expect(screen.getByRole("heading", { level: 2, name: /november 2026/i })).toBeInTheDocument();
    expect(screen.getByText(s.pickDay)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /15\. novembra: 3 podujatia/ }));
    await userEvent.click(screen.getByRole("button", { name: s.previousMonth }));
    await userEvent.click(screen.getByRole("button", { name: s.previousMonth }));
    expect(screen.getByRole("heading", { level: 2, name: /september 2026/i })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: s.thisMonth }));
    expect(screen.getByRole("heading", { level: 2, name: /október 2026/i })).toBeInTheDocument();
  });

  it("names three events of a busy day and counts the rest", () => {
    const [first] = loadedEvents();
    const busy = Array.from({ length: 5 }, (_, at) => ({ ...first, id: `${first.id}-${at}`, name: `Podujatie ${at}` }));
    render(<MonthView events={busy} picked={null} onPick={() => undefined} s={s} />);
    expect(screen.getByText(s.more(2))).toBeInTheDocument();
    expect(screen.getByText("Podujatie 2")).toBeInTheDocument();
    expect(screen.queryByText("Podujatie 3")).toBeNull();
    everyDay();
  });

  it("gives the whole region's calendar, and the chosen dates as a second link, to copy or open", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    show();
    await loaded();
    const region = screen.getByRole("textbox", { name: new RegExp(s.wholeRegion) });
    expect(region).toHaveValue(ICS);
    expect(screen.queryByText(s.onlyDates)).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: s.copy })[0]);
    expect(writeText).toHaveBeenCalledWith(ICS);
    expect(await screen.findByText(s.copied)).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: s.openCalendar })[0]).toHaveAttribute("href", ICS.replace("http://", "webcal://"));
    fireEvent.change(screen.getByLabelText(s.from), { target: { value: "2026-11-01" } });
    fireEvent.change(screen.getByLabelText(s.to), { target: { value: "2026-11-30" } });
    expect(screen.getByRole("textbox", { name: new RegExp(s.dateRange) })).toHaveValue(`${ICS}?from=2026-11-01&to=2026-11-30`);
    expect(screen.getByText(s.onlyDates)).toBeInTheDocument();
    fireEvent.focus(region);
  });

  it("says so when the link cannot be copied, and offers no date link for a range that holds no day", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: vi.fn(async () => Promise.reject(new Error("denied"))) } });
    render(<Subscribe slug={SLUG} filters={{ ...NO_FILTERS, from: "2026-11-02", to: "2026-11-01", town: "Zvolen" }} s={s} />);
    expect(screen.queryByRole("textbox", { name: new RegExp(s.dateRange) })).toBeNull();
    expect(screen.getByText(s.onlyDates)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: s.copy }));
    expect(await screen.findByText(s.copyFailed)).toBeInTheDocument();
  });

  it("says when there are no upcoming events yet", async () => {
    show({ rows: [] });
    expect(await screen.findByText(s.noEvents)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: new RegExp(s.wholeRegion) })).toHaveValue(ICS);
  });

  it("says why the events could not be loaded, refused or offline", async () => {
    show({ fail: true });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("the policy does not grant this type");
    // Asking again never grants a right.
    expect(within(alert).queryByRole("button")).toBeNull();
  });

  it("says the reason of a request that fails on its way", async () => {
    show({ fail: new TypeError("offline") });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("offline");
    vi.stubGlobal("fetch", vi.fn(async (path: string) => new Response(JSON.stringify(answer(new URL(path, "http://portal.test"), EVENTS)), { status: 200, headers: { "content-type": "application/json" } })));
    fireEvent.click(within(alert).getByRole("button"));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("holds at most five thousand events and says the rest are cut off", async () => {
    const asked: number[] = [];
    const source = {
      query: async (_query: unknown, page: { offset: number; limit: number }) => {
        asked.push(page.offset);
        return { rows: Array.from({ length: page.limit }, (_, at) => toRichRow({ ...EVENTS[0], id: `${EVENTS[0].id}-${page.offset + at}` })) };
      },
    } as unknown as EntitySource;
    const result = await loadEvents(source, "sk");
    expect(asked).toEqual([0, 1000, 2000, 3000, 4000]);
    expect(result).toMatchObject({ status: "ready", truncated: true });
    expect(MOST).toBe(5000);
  });

  it("says it has nothing to read when the app has no endpoint of the events space", () => {
    show({ withEndpoint: false });
    expect(screen.getByText(s.noEndpoint)).toBeInTheDocument();
  });

  it("speaks English to an English reader", async () => {
    show({ language: "en" });
    await loaded();
    expect(screen.getByText(LOCALES.en.results(EVENTS.length - 1))).toHaveAttribute("role", "status");
    expect(screen.getByRole("heading", { name: LOCALES.en.subscribe })).toBeInTheDocument();
  });

  for (const language of ["sk", "en"]) {
    it(`uses every control of every view, a calendar month at a time (${language})`, async () => {
      const writeText = vi.fn(async () => undefined);
      vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
      const t = LOCALES[language as "sk" | "en"];
      show({ language });
      await loaded();
      // The filters, then the calendar links the dates add.
      useEvery(document.querySelector(".filters") as HTMLElement);
      fireEvent.change(screen.getByLabelText(t.from), { target: { value: "2026-11-01" } });
      fireEvent.change(screen.getByLabelText(t.to), { target: { value: "2026-11-30" } });
      useEvery(document.querySelector(".side") as HTMLElement);
      fireEvent.click(screen.getByRole("button", { name: t.clear }));
      for (const view of [t.view.map, t.view.list]) {
        fireEvent.click(tab(view));
        useEvery(document.querySelector(".jc-tabpanel") as HTMLElement);
      }
      fireEvent.click(tab(t.view.calendar));
      everyDay();
      fireEvent.click(screen.getByRole("button", { name: t.previousMonth }));
      everyDay();
      for (let month = 0; month < 3; month += 1) {
        fireEvent.click(screen.getByRole("button", { name: t.nextMonth }));
        everyDay();
      }
      fireEvent.click(screen.getByRole("button", { name: t.thisMonth }));
      expect(writeText).toHaveBeenCalled();
    }, 60_000);
  }

  it("names its source and has nothing axe finds, in each view", async () => {
    const { container } = show();
    await loaded();
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    for (const view of [s.view.map, s.view.list, s.view.calendar]) {
      await userEvent.click(tab(view));
      const found = await axe.run(container, { rules: { region: { enabled: false } } });
      expect(found.violations.map((v) => v.id)).toEqual([]);
    }
  });
});
