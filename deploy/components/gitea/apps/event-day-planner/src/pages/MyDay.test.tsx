import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { EVENTS } from "../fixtures/events";
import { ServerProblem } from "../share";
import type { ShareApi } from "../share";
import { AccessProvider } from "../access";
import { memory, STEWARD, VIEWER } from "../fixtures/memory";
import type { Me } from "../share";
import { hourOf, MyDay } from "./MyDay";

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    remove = vi.fn();
  },
  LngLatBounds: class {
    extend = vi.fn();
  },
  Popup: class {},
  setWorkerUrl: vi.fn(),
}));
/** What a click on a bar of the hours chart hands the page, as ECharts would. */
let clickBar: (params: { name?: unknown }) => void = () => undefined;
vi.mock("echarts/core", () => ({
  use: vi.fn(),
  init: vi.fn(() => ({
    setOption: vi.fn(),
    resize: vi.fn(),
    dispose: vi.fn(),
    on: (event: string, handler: typeof clickBar) => {
      if (event === "click") clickBar = handler;
    },
  })),
}));
const downloads: Array<{ blob: Blob; name: string }> = [];
vi.mock("@joinedcontext/sdk", async (original) => ({
  ...(await original<typeof import("@joinedcontext/sdk")>()),
  download: (blob: Blob, name: string) => downloads.push({ blob, name }),
}));

/** The App's server with nothing shared and a viewer signed in: most of these tests are about the planner on the page. */
const shares = (): ShareApi => memory();
let api = shares();

const READ = {
  permissions: [{ resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function show(language = "en", entities = EVENTS) {
  const client = stubClient({ entities, access: READ }, { appName: "event-day-planner", language });
  render(
    <JcProvider client={client}>
      <MyDay shares={api} />
    </JcProvider>,
  );
  return client;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2030-10-20T05:00:00Z"));
  window.history.replaceState(null, "", "/");
  downloads.length = 0;
  api = shares();
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

// T-3329: the first screen answers "what can I see today" without a click.
describe("MyDay", () => {
  it("suggests a day of events that follow one another, read only through the app's endpoint", async () => {
    const client = show();
    const plan = await screen.findByRole("list", { name: "The plan" });
    expect(screen.getByText(/A suggested day/)).toBeInTheDocument();
    expect(within(plan).getAllByRole("listitem").length).toBeGreaterThan(1);
    expect(within(plan).getByText("Workshop for Families")).toBeInTheDocument();
    expect(client.transport.calls.every((call) => call.method === "GET" && call.path.includes("/api/endpoint/"))).toBe(true);
  });

  it("plans the events a visitor picks, flags a clash, and keeps the picks in the address", async () => {
    show();
    const list = await screen.findByRole("list", { name: "Events of the day" });
    fireEvent.click(within(list).getByRole("checkbox", { name: /Organ concert/ }));
    fireEvent.click(within(list).getByRole("checkbox", { name: /Poetry reading/ }));
    await waitFor(() => expect(screen.getByText("2 chosen events in their best order.")).toBeInTheDocument());
    expect(screen.getByText("Organ concert and Poetry reading take place at the same time.")).toBeInTheDocument();
    expect(screen.getByText(/min late/)).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("pick")).toBe("helsinki-agf2,helsinki-agf3");
  });

  // T-3347: the picked day shared through the App's server, and a shared link opened again.
  it("shares the picked day with the events' full ids", async () => {
    show();
    const list = await screen.findByRole("list", { name: "Events of the day" });
    fireEvent.click(within(list).getByRole("checkbox", { name: /Organ concert/ }));
    fireEvent.click(within(list).getByRole("checkbox", { name: /Poetry reading/ }));
    await waitFor(() => expect(screen.getByText("2 chosen events in their best order.")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Share this day" }));
    await waitFor(() => expect(api.share).toHaveBeenCalledWith("2030-10-20", ["urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-agf2", "urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-agf3"], "en"));
    expect(((await screen.findByLabelText("Link to the shared day")) as HTMLInputElement).value).toContain("?share=abc123def456");
  });

  it("opens a shared link as its day and picks, and drops the code from the address", async () => {
    window.history.replaceState(null, "", "/?share=abc123def456");
    show();
    await waitFor(() => expect(screen.getByText("Shared day opened: 2 events, planned from the data of now.")).toBeInTheDocument());
    expect(api.get).toHaveBeenCalledWith("abc123def456");
    const params = new URLSearchParams(window.location.search);
    expect(params.get("pick")).toBe("helsinki-agf2,helsinki-agf3");
    expect(params.get("share")).toBeNull();
    await waitFor(() => expect(screen.getByText("2 chosen events in their best order.")).toBeInTheDocument());
  });

  it("says so when a shared link names no day", async () => {
    api.get = vi.fn(async () => {
      throw new ServerProblem(404, "no day is shared under this link");
    });
    window.history.replaceState(null, "", "/?share=zzzzzzzzzzzz");
    show();
    expect(await screen.findByText(/No day is shared under this link/)).toBeInTheDocument();
  });

  it("never asks the server for a code it could not have made", async () => {
    window.history.replaceState(null, "", "/?share=..%2F..%2Fx");
    show();
    expect(await screen.findByText(/No day is shared under this link/)).toBeInTheDocument();
    expect(api.get).not.toHaveBeenCalled();
    expect(new URLSearchParams(window.location.search).get("share")).toBeNull();
  });

  it("downloads the day as a calendar file", async () => {
    show();
    await screen.findByRole("list", { name: "The plan" });
    fireEvent.click(screen.getByRole("button", { name: "Download to calendar (.ics)" }));
    expect(downloads).toHaveLength(1);
    expect(downloads[0].name).toBe("2030-10-20-helsinki.ics");
    expect(await downloads[0].blob.text()).toContain("BEGIN:VCALENDAR");
  });

  it("lists a cancelled event without letting it be picked", async () => {
    show();
    const list = await screen.findByRole("list", { name: "Events of the day" });
    expect(within(list).getByRole("checkbox", { name: /Cancelled lecture/ })).toBeDisabled();
    expect(within(list).getByText("Cancelled")).toBeInTheDocument();
  });

  it("narrows the list to an hour from the address, and a click on the chip shows every hour", async () => {
    window.history.replaceState(null, "", "/?hour=18");
    show();
    const list = await screen.findByRole("list", { name: "Events of the day" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Show every hour, not only at 18" }));
    await waitFor(() => expect(within(screen.getByRole("list", { name: "Events of the day" })).getAllByRole("listitem")).toHaveLength(6));
  });

  it("speaks Finnish when the Portal does", async () => {
    show("fi");
    expect(await screen.findByRole("list", { name: "Suunnitelma" })).toBeInTheDocument();
    expect(screen.getByText(/Ehdotettu päivä/)).toBeInTheDocument();
  });

  it("says so when the day has no event, and in words when the events cannot be read", async () => {
    show("en", []);
    expect(await screen.findAllByText("No event takes place on this day.")).not.toHaveLength(0);
  });

  it("offers to try again when the endpoint fails", async () => {
    const client = stubClient(
      { entities: EVENTS, access: READ, refuse: () => ({ status: 503, body: { title: "Unavailable" } }) },
      { appName: "event-day-planner", language: "en" },
    );
    render(
      <JcProvider client={client}>
        <MyDay shares={api} />
      </JcProvider>,
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The events could not be read (HTTP 503). Try again later.");
    const reads = client.transport.calls.length;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(client.transport.calls.length).toBeGreaterThan(reads));
  });

  // T-3373: every control of the day answers, in either language: each event picked and let go,
  // each name pressed (outside the shell there is no panel to open), the day, the search, the
  // source, the calendar and Clear.
  for (const [language, words] of [
    ["en", { day: "Day", search: "Search", clear: "Clear choices", ics: "Download to calendar (.ics)", source: "Source", list: "Events of the day", monday: "Monday matinee" }],
    ["fi", { day: "Päivä", search: "Haku", clear: "Tyhjennä valinnat", ics: "Lataa kalenteriin (.ics)", source: "Lähde", list: "Päivän tapahtumat", monday: "Maanantain matinea" }],
    ["sv", { day: "Dag", search: "Sök", clear: "Rensa valen", ics: "Ladda ner till kalendern (.ics)", source: "Källa", list: "Dagens evenemang", monday: "Monday matinee" }],
  ] as const) {
    it(`answers every control of the day in ${language}`, async () => {
      show(language);
      const sweep = async () => {
        const list = await screen.findByRole("list", { name: words.list });
        for (const box of within(list).getAllByRole("checkbox")) {
          if ((box as HTMLInputElement).disabled) continue;
          fireEvent.click(box);
          fireEvent.click(box);
        }
        for (const button of within(list).getAllByRole("button")) fireEvent.click(button);
        for (const link of within(list).queryAllByRole("link", { name: words.source })) fireEvent.click(link);
      };
      await sweep();
      expect(screen.queryByRole("dialog")).toBeNull();
      fireEvent.click(await screen.findByRole("button", { name: words.ics }));
      expect(downloads).toHaveLength(1);

      fireEvent.change(screen.getByLabelText(words.day), { target: { value: "2030-10-21" } });
      expect(await screen.findByText(words.monday, { selector: "button" })).toBeInTheDocument();
      expect(new URLSearchParams(window.location.search).get("day")).toBe("2030-10-21");
      await sweep();
      fireEvent.change(screen.getByRole("searchbox", { name: words.search }), { target: { value: "zzz" } });
      expect(screen.queryByRole("list", { name: words.list })).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: words.clear }));
      expect(window.location.search).toBe("");
      expect(await screen.findByRole("list", { name: words.list })).toBeInTheDocument();
    });
  }
});

// T-3329: what the page does at the edges of a day.
describe("MyDay at the edges", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("narrows the list to the hour of a clicked bar, and a second click on it shows every hour", async () => {
    show();
    const list = await screen.findByRole("list", { name: "Events of the day" });
    await waitFor(() => expect(screen.getByRole("img", { name: "Events starting each hour" })).toBeInTheDocument());
    act(() => clickBar({}));
    act(() => clickBar({ name: 12 }));
    expect(new URLSearchParams(window.location.search).get("hour")).toBe("12");
    // 12 in Helsinki: the organ concert and the poetry reading.
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
    act(() => clickBar({ name: 12 }));
    expect(new URLSearchParams(window.location.search).get("hour")).toBeNull();
  });

  it("marks what the visitor can no longer reach late in the day, and has no calendar to give for it", async () => {
    vi.setSystemTime(new Date("2030-10-20T16:30:00Z"));
    window.history.replaceState(null, "", "/?pick=helsinki-agf2");
    show();
    expect(await screen.findByText("out of reach")).toBeInTheDocument();
    const ics = screen.getByRole("button", { name: "Download to calendar (.ics)" });
    expect(ics).toHaveAttribute("aria-disabled", "true");
    expect(ics).toHaveAttribute("title", "The day holds no event to download.");
    fireEvent.click(ics);
    expect(downloads).toEqual([]);
  });

  it("plans an event with no place without a walk to it", async () => {
    const placeless = Object.fromEntries(Object.entries(EVENTS[0]).filter(([attr]) => attr !== "location")) as Row;
    show("en", [placeless]);
    expect(await screen.findByText("no place: walking not counted")).toBeInTheDocument();
  });

  it("reads a day the address cannot name as today, and an emptied date as today too", async () => {
    window.history.replaceState(null, "", "/?day=someday");
    show();
    const day = await screen.findByLabelText("Day");
    expect(day).toHaveValue("2030-10-20");
    fireEvent.change(day, { target: { value: "" } });
    expect(day).toHaveValue("2030-10-20");
    // Enter in the search sends nothing anywhere.
    fireEvent.submit(screen.getByRole("search"));
    expect(screen.getByRole("list", { name: "Events of the day" })).toBeInTheDocument();
  });

  it("says in words when the planner fails", async () => {
    class StoppingWorker {
      onmessage: unknown = null;
      onerror: (() => void) | null = null;
      postMessage() {
        queueMicrotask(() => this.onerror?.());
      }
    }
    vi.stubGlobal("Worker", StoppingWorker);
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent("The day could not be planned: The planner stopped. Reload the page.");
  });

  it("says the connection failed when no answer came at all", async () => {
    const client = stubClient(
      {
        entities: EVENTS,
        access: READ,
        refuse: () => {
          throw new TypeError("Failed to fetch");
        },
      },
      { appName: "event-day-planner", language: "en" },
    );
    render(
      <JcProvider client={client}>
        <MyDay shares={api} />
      </JcProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("The events could not be read. Check the connection and try again.");
  });
});

/** The page for `me`, with the App's server deciding as the real one does. */
function showAs(me: Me | null, server = memory(me), language = "en") {
  const client = stubClient({ entities: EVENTS, access: READ }, { appName: "event-day-planner", language });
  render(
    <JcProvider client={client}>
      <AccessProvider api={server}>
        <MyDay shares={server} />
      </AccessProvider>
    </JcProvider>,
  );
  return server;
}
const ORGAN = "urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-agf2";
const MONDAY = "urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-agf7";

// T-3826: a steward features events every visitor sees, with a note; a viewer picks them.
describe("MyDay featured events", () => {
  it("lets a steward feature an event, note it and take it back, each shown at once", async () => {
    const server = showAs(STEWARD);
    const list = await screen.findByRole("list", { name: "Events of the day" });
    expect(await screen.findByText("No event is featured.")).toBeInTheDocument();
    fireEvent.click(within(list).getByRole("button", { name: "Feature: Organ concert" }));
    const featured = await screen.findByRole("list", { name: "Featured events" });
    expect(within(featured).getByText("Organ concert")).toBeInTheDocument();
    expect(await screen.findByText("Featured: Organ concert.")).toBeInTheDocument();
    expect(within(list).getByText("Featured")).toBeInTheDocument();
    expect(server.feature).toHaveBeenCalledWith(ORGAN, "");

    const note = await within(featured).findByRole("textbox", { name: "Note" });
    const save = within(featured).getByRole("button", { name: "Save the note" });
    expect(save).toBeDisabled();
    fireEvent.change(note, { target: { value: "  Free entry  " } });
    fireEvent.click(save);
    expect(await screen.findByText("The note is saved.")).toBeInTheDocument();
    expect(server.changeNote).toHaveBeenCalledWith(1, "Free entry");

    // Picked from the card: the planner takes it like any other pick.
    fireEvent.click(within(featured).getByRole("checkbox", { name: "Add to my day: Organ concert" }));
    expect(new URLSearchParams(window.location.search).get("pick")).toBe("helsinki-agf2");

    fireEvent.click(within(featured).getByRole("button", { name: "Remove from featured: Organ concert" }));
    expect(await screen.findByText("No longer featured: Organ concert.")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Featured events" })).toBeNull();
    expect(server.featured_).toEqual([]);

    // Every event of the day can be featured from the list.
    for (const button of within(list).getAllByRole("button", { name: /^Feature: / })) fireEvent.click(button);
    await waitFor(() => expect(server.featured_.length).toBeGreaterThan(3));
    expect(within(list).queryAllByRole("button", { name: /^Feature: / })).toEqual([]);
    // Each one picked from the card and let go, then taken back.
    const all = await screen.findByRole("list", { name: "Featured events" });
    for (const box of within(all).getAllByRole("checkbox")) {
      fireEvent.click(box);
      fireEvent.click(box);
    }
    for (const button of within(all).getAllByRole("button", { name: /^Remove from featured: / })) {
      await waitFor(() => expect(button).toBeEnabled());
      fireEvent.click(button);
    }
    expect(await screen.findByText("No event is featured.")).toBeInTheDocument();
  });

  // T-4783: a note typed while the server is still featuring the event survives the server's answer.
  it("keeps a note typed before the feature is confirmed, and saves it", async () => {
    const server = memory(STEWARD);
    const real = server.feature;
    let confirm = () => {};
    server.feature = vi.fn((eventId: string, note: string) => new Promise((done) => (confirm = () => done(real(eventId, note)))));
    showAs(STEWARD, server);
    const list = await screen.findByRole("list", { name: "Events of the day" });
    await screen.findByText("No event is featured.");
    fireEvent.click(within(list).getByRole("button", { name: "Feature: Organ concert" }));
    const featured = await screen.findByRole("list", { name: "Featured events" });
    const note = within(featured).getByRole("textbox", { name: "Note" });
    fireEvent.change(note, { target: { value: "Free entry" } });
    await act(async () => confirm());
    expect(await screen.findByText("Featured: Organ concert.")).toBeInTheDocument();
    expect(within(featured).getByRole("textbox", { name: "Note" })).toHaveValue("Free entry");
    const save = within(featured).getByRole("button", { name: "Save the note" });
    expect(save).toBeEnabled();
    fireEvent.click(save);
    expect(await screen.findByText("The note is saved.")).toBeInTheDocument();
    expect(server.changeNote).toHaveBeenCalledWith(1, "Free entry");
  });

  it("puts the list back and says why when the server refuses a steward's change", async () => {
    const server = memory(STEWARD);
    await server.feature(ORGAN, "Free entry");
    server.unfeature = vi.fn(async () => {
      throw new ServerProblem(404, "no such featured event");
    });
    server.feature = vi.fn(async () => {
      throw new ServerProblem(403, "this needs the role steward");
    });
    showAs(STEWARD, server);
    const featured = await screen.findByRole("list", { name: "Featured events" });
    fireEvent.click(within(featured).getByRole("button", { name: "Remove from featured: Organ concert" }));
    expect(await screen.findByText(/no longer there, or it is not yours/)).toBeInTheDocument();
    const back = screen.getByRole("list", { name: "Featured events" });
    expect(within(back).getByText("Organ concert")).toBeInTheDocument();

    const list = screen.getByRole("list", { name: "Events of the day" });
    fireEvent.click(within(list).getByRole("button", { name: "Feature: Poetry reading" }));
    expect(await within(list).findByText(/Your account may not do this/)).toBeInTheDocument();
    expect(within(back).queryByText("Poetry reading")).toBeNull();
  });

  it("shows a viewer the featured events and their notes, with nothing to change", async () => {
    const server = memory(STEWARD);
    await server.feature(ORGAN, "Free entry");
    await server.feature(MONDAY, "");
    await server.feature("urn:ngsi-ld:Event:hel.fi:helsinki:gone", "");
    showAs(VIEWER, Object.assign(server, { me: vi.fn(async () => VIEWER) }));
    const featured = await screen.findByRole("list", { name: "Featured events" });
    expect(screen.getByText("A steward curates the featured events.")).toBeInTheDocument();
    expect(within(featured).getByText("Free entry")).toBeInTheDocument();
    // On another day it says so, with no pick; one the page never read goes by its id.
    expect(within(featured).getAllByText(/not on this day/)).toHaveLength(2);
    expect(within(featured).getByText("gone")).toBeInTheDocument();
    expect(within(featured).getAllByRole("checkbox")).toHaveLength(1);
    expect(within(featured).queryByRole("button")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Feature:/ })).toBeNull();
  });

  it("says when the featured events cannot be read, and the day still plans", async () => {
    const server = memory(null);
    server.featured = vi.fn(async () => {
      throw new ServerProblem(502, "");
    });
    showAs(null, server, "fi");
    expect(await screen.findByText("Palvelin ei vastannut. Yritä hetken päästä uudelleen.")).toBeInTheDocument();
    expect(screen.getByText("Suositeltuja tapahtumia ei ole.")).toBeInTheDocument();
    expect(await screen.findByRole("list", { name: "Suunnitelma" })).toBeInTheDocument();
  });
});

describe("hourOf", () => {
  it("takes an hour only in the day", () => {
    expect(hourOf("0")).toBe(0);
    expect(hourOf("23")).toBe(23);
    for (const wrong of ["", "24", "-1", "1.5", "noon"]) expect(hourOf(wrong)).toBeNull();
  });
});
