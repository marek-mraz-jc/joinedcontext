/**
 * T-3828: who reads, and what they change. A signed-in reader keeps comparisons of their own and
 * shares one by its link, which another reader opens read-only; a steward pins the comparison the
 * page opens on for everyone. Each change shows at once and is taken back when the server refuses;
 * the Changes page shows the history.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import wasm from "../wasm/pkg/district_compare_bg.wasm?url&inline";
import { compare, initSync } from "../wasm/pkg/district_compare.js";
import App from "./App";
import { shareLink } from "./components/Saved";
import { ComparerContext, parseAnswer } from "./compare";
import type { Comparer } from "./compare";
import { ENTITIES } from "./fixtures/districts";
import { ServerContext } from "./server";
import { memory, STEWARD, token, VIEWER } from "./testing/server";
import type { Memory } from "./testing/server";
import { sweep } from "./testing/sweep";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    once = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

initSync({ module: Uint8Array.from(atob(wasm.slice(wasm.indexOf(",") + 1)), (c) => c.charCodeAt(0)) });
const COMPARER: Comparer = async (input) => parseAnswer(compare(JSON.stringify(input)));

const TYPES = ["AdministrativeArea", "Event", "BikeHireDockingStation", "Alert", "AirQualityObserved"];
const ACCESS = {
  permissions: TYPES.map((type) => ({ resource: { type }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const })),
  prohibitions: [],
};

let server: Memory;

function show(language = "en", search = "") {
  window.history.replaceState(null, "", `/?lang=${language}${search}`);
  const client = stubClient({ entities: ENTITIES, access: ACCESS }, { appName: "district-compare" });
  return render(
    <JcProvider client={client}>
      <ServerContext.Provider value={server}>
        <ComparerContext.Provider value={COMPARER}>
          <App />
        </ComparerContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
}

const open = async (page: string) => fireEvent.click(await screen.findByRole("button", { name: page }));
const measure = () => screen.getByRole("combobox", { name: "Measure" });
const compared = async () => {
  const table = await screen.findByRole("table", { name: "Selected districts compared" });
  return within(table)
    .getAllByRole("button", { name: /^Deselect / })
    .map((b) => b.textContent?.replace(" ×", ""));
};

beforeEach(() => {
  server = memory();
  window.location.hash = "";
});
afterEach(() => {
  window.history.replaceState(null, "", "/");
  window.location.hash = "";
});

describe("a signed-out reader", () => {
  it("opens on the pinned comparison, keeps nothing, and every closed control says why", async () => {
    server.pins.push({ id: 7, name: "Air", codes: ["103"], measure: "pm25", ownerName: "Sami Steward" });
    show();
    expect(await screen.findByLabelText("Role: Signed out")).toBeInTheDocument();
    expect(await screen.findByTestId("pinned")).toHaveTextContent("Air, pinned by Sami Steward. The page opens on it.");
    await waitFor(async () => expect(await compared()).toEqual(["Töölö"]));
    expect(measure()).toHaveValue("pm25");
    expect(screen.getByRole("button", { name: "Keep this comparison" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Keep this comparison" })).toHaveAttribute("title", "Sign in to keep comparisons.");
    expect(screen.getByRole("button", { name: "Pin this comparison for everyone" })).toHaveAttribute("title", "Only a steward pins a comparison for everyone.");
    expect(server.comparisons).not.toHaveBeenCalled();
    await open("Changes");
    expect(await screen.findByText("Sign in to see the changes.")).toBeInTheDocument();
  });

  it("is asked to sign in to open a shared comparison", async () => {
    show("en", `&comparison=${token(5)}`);
    expect(await screen.findByText("Sign in to open a shared comparison.")).toBeInTheDocument();
    expect(server.shared).not.toHaveBeenCalled();
  });
});

describe("a viewer", () => {
  beforeEach(() => {
    server.who = VIEWER;
  });

  it("keeps the comparison on screen, opens, updates and deletes it, at once on screen; History shows their own", async () => {
    show();
    expect(await screen.findByLabelText("Role: Viewer")).toBeInTheDocument();
    expect(await screen.findByText("You have not kept a comparison yet.")).toBeInTheDocument();
    const before = await compared();
    expect(before).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", { name: "Keep this comparison" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Give the comparison a name (at most 80 characters).");
    expect(server.keep).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Name of the comparison"), { target: { value: "  My two  " } });
    fireEvent.click(screen.getByRole("button", { name: "Keep this comparison" }));
    const list = await screen.findByRole("list", { name: "Saved comparisons" });
    expect(within(list).getByText("My two")).toBeInTheDocument();
    expect(await screen.findByText("Kept: My two")).toBeInTheDocument();
    expect(server.keep).toHaveBeenCalledWith({ name: "My two", codes: expect.any(Array), measure: "events" });
    const link = within(list).getByRole("link", { name: "Link to share: My two" });
    expect(link.getAttribute("href")).toMatch(new RegExp(`\\?comparison=${token(100)}&lang=en$`));
    // Following it opens the page anew; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);

    // Another view, then back to the kept one.
    fireEvent.click(screen.getByRole("button", { name: "Clear selection" }));
    expect(within(list).getByRole("button", { name: "Update My two to what is on screen" })).toHaveAttribute("title", "Select districts first.");
    fireEvent.change(measure(), { target: { value: "alerts" } });
    fireEvent.click(within(list).getByRole("button", { name: "Open My two" }));
    expect(await compared()).toEqual(before);
    expect(measure()).toHaveValue("events");

    fireEvent.change(measure(), { target: { value: "bikes" } });
    fireEvent.click(within(list).getByRole("button", { name: "Update My two to what is on screen" }));
    expect(await screen.findByText("Updated: My two")).toBeInTheDocument();
    expect(server.kept[0].measure).toBe("bikes");
    expect(within(list).getByText(/^City-bike stations:/)).toBeInTheDocument();

    fireEvent.click(within(list).getByRole("button", { name: "Delete My two" }));
    expect(await screen.findByText("Deleted: My two")).toBeInTheDocument();
    expect(server.kept).toHaveLength(0);

    await open("Changes");
    expect(await screen.findByText("Your own changes, newest first.")).toBeInTheDocument();
    const rows = within(await screen.findByRole("table", { name: "Changes" })).getAllByRole("row");
    expect(rows.map((row) => row.textContent)).toEqual([
      "WhenWhoWhat",
      expect.stringMatching(/Vilja ViewerRemoved: deleted a comparison$/),
      expect.stringMatching(/Vilja ViewerChanged: changed the comparison My two$/),
      expect.stringMatching(/Vilja ViewerAdded: kept the comparison My two$/),
    ]);
  });

  it("takes a refused change back and says why", async () => {
    server.kept.push({ id: 3, name: "Mine", codes: ["101"], measure: "events", ownerName: VIEWER.name, owner: VIEWER.id });
    show();
    const list = await screen.findByRole("list", { name: "Saved comparisons" });
    server.forget = vi.fn(async () => {
      throw new Error("no such record, or not yours to change");
    });
    fireEvent.click(within(list).getByRole("button", { name: "Delete Mine" }));
    expect(await screen.findByText("Not saved: no such record, or not yours to change")).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Saved comparisons" })).getByText("Mine")).toBeInTheDocument();

    server.keep = vi.fn(async () => {
      throw new Error("at most 20 comparisons are kept; delete one first");
    });
    fireEvent.change(screen.getByLabelText("Name of the comparison"), { target: { value: "One more" } });
    fireEvent.click(screen.getByRole("button", { name: "Keep this comparison" }));
    expect(await screen.findByText("Not saved: at most 20 comparisons are kept; delete one first")).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Saved comparisons" })).queryByText("One more")).toBeNull();

    server.change = vi.fn(async () => {
      throw new Error("no such record, or not yours to change");
    });
    fireEvent.change(measure(), { target: { value: "alerts" } });
    fireEvent.click(screen.getByRole("button", { name: "Update Mine to what is on screen" }));
    expect(await screen.findAllByText("Not saved: no such record, or not yours to change")).toHaveLength(1);
    expect(within(screen.getByRole("list", { name: "Saved comparisons" })).getByText(/^Events:/)).toBeInTheDocument();

        // A viewer cannot pin: the server refuses even when the button is reached.
    expect(screen.getByRole("button", { name: "Pin this comparison for everyone" })).toBeDisabled();
  });

  it("opens a comparison another reader shared, read-only", async () => {
    server.kept.push({ id: 5, share: token(5), name: "Kallio only", codes: ["102"], measure: "alerts", ownerName: "Olli Other", owner: "u-other" });
    show("en", `&comparison=${token(5)}`);
    expect(await screen.findByText("Shared comparison Kallio only, by Olli Other: read-only.")).toBeInTheDocument();
    await waitFor(async () => expect(await compared()).toEqual(["Kallio"]));
    expect(measure()).toHaveValue("alerts");
    expect(screen.queryByRole("button", { name: "Delete Kallio only" })).toBeNull();
  });

  it("never opens a comparison by its id: a link naming one is no shared link", async () => {
    server.kept.push({ id: 5, share: token(5), name: "Kallio only", codes: ["102"], measure: "alerts", ownerName: "Olli Other", owner: "u-other" });
    show("en", "&comparison=5");
    expect(await screen.findByLabelText("Role: Viewer")).toBeInTheDocument();
    expect(server.shared).not.toHaveBeenCalled();
    expect(screen.queryByText(/Shared comparison/)).toBeNull();
  });

  it("says when a shared comparison is gone, and when the own ones cannot be read", async () => {
    server.comparisons = vi.fn(async () => {
      throw new Error("the database gave no answer");
    });
    show("en", `&comparison=${token(99)}`);
    expect(await screen.findByText("The shared comparison could not be opened: no such comparison")).toBeInTheDocument();
    expect(await screen.findByText("The comparisons could not be read: the database gave no answer")).toBeInTheDocument();
  });
});

describe("a steward", () => {
  beforeEach(() => {
    server.who = STEWARD;
  });

  it("pins the comparison on screen for everyone, then takes it back to the older pin; the history shows everyone's", async () => {
    server.pins.push({ id: 1, name: "Older", codes: ["101"], measure: "events", ownerName: "Sami Steward" });
    show();
    expect(await screen.findByLabelText("Role: Steward")).toBeInTheDocument();
    await screen.findByTestId("pinned");
    expect(screen.getByRole("button", { name: "Take the pin back" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Pin this comparison for everyone" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Give the comparison a name");
    fireEvent.change(screen.getByLabelText("Name of the comparison"), { target: { value: "Everyone's" } });
    fireEvent.change(measure(), { target: { value: "bikeSlots" } });
    fireEvent.click(screen.getByRole("button", { name: "Pin this comparison for everyone" }));
    expect(screen.getByTestId("pinned")).toHaveTextContent("Everyone's, pinned by Sami Steward.");
    expect(await screen.findByText("Pinned: Everyone's")).toBeInTheDocument();
    expect(server.pins[0]).toMatchObject({ name: "Everyone's", measure: "bikeSlots" });

    fireEvent.click(screen.getByRole("button", { name: "Take the pin back" }));
    expect(await screen.findByText("The pin is taken back.")).toBeInTheDocument();
    expect(screen.getByTestId("pinned")).toHaveTextContent("Older, pinned by Sami Steward.");
    fireEvent.click(screen.getByRole("button", { name: "Open the pinned one" }));
    expect(await compared()).toEqual(["Kamppi"]);

    // A refused pin is taken back.
    server.pin = vi.fn(async () => {
      throw new Error("this needs the role steward");
    });
    fireEvent.change(screen.getByLabelText("Name of the comparison"), { target: { value: "Refused" } });
    fireEvent.click(screen.getByRole("button", { name: "Pin this comparison for everyone" }));
    expect(await screen.findByText("Not saved: this needs the role steward")).toBeInTheDocument();
    expect(screen.getByTestId("pinned")).toHaveTextContent("Older, pinned by Sami Steward.");

    server.unpin = vi.fn(async () => {
      throw new Error("nothing is pinned");
    });
    fireEvent.click(screen.getByRole("button", { name: "Take the pin back" }));
    expect(await screen.findByText("Not saved: nothing is pinned")).toBeInTheDocument();
    expect(screen.getByTestId("pinned")).toHaveTextContent("Older, pinned by Sami Steward.");

    await open("Changes");
    expect(await screen.findByText("Everyone's changes, newest first.")).toBeInTheDocument();
    expect(within(await screen.findByRole("table", { name: "Changes" })).getAllByRole("row")).toHaveLength(3);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(server.changes).toHaveBeenCalledTimes(2));
  });

  for (const [language, pages] of [
    ["en", ["Changes", "Compare districts"]],
    ["fi", ["Muutokset", "Vertaa kaupunginosia"]],
    ["sv", ["Ändringar", "Jämför stadsdelar"]],
  ] as const) {
    it(`uses every control of every page once, in ${language}`, async () => {
      server.kept.push({ id: 3, name: "Mine", codes: ["101", "102"], measure: "events", ownerName: STEWARD.name, owner: STEWARD.id });
      server.pins.push({ id: 1, name: "Older", codes: ["101"], measure: "events", ownerName: "Sami Steward" });
      const { container } = show(language);
      await waitFor(() => expect(server.comparisons).toHaveBeenCalled());
      await waitFor(() => expect(container.querySelector(".app-saved")).not.toBeNull());
      const main = () => container.querySelector<HTMLElement>("main") ?? container;
      await sweep(main());
      for (const page of pages) {
        await open(page);
        await act(async () => new Promise((done) => setTimeout(done, 0)));
        await sweep(main());
      }
      // Each district alone once, its Details and then its deselect button pressed.
      for (const box of [...container.querySelectorAll<HTMLInputElement>(".app-ranked-list input")].map((b) => b.getAttribute("aria-label")!)) {
        const clear = container.querySelector<HTMLButtonElement>(".app-filter-bar button");
        if (clear) fireEvent.click(clear);
        fireEvent.click(screen.getByRole("checkbox", { name: box }));
        for (const kind of [".app-open", ".app-remove-district"]) {
          const button = await waitFor(() => container.querySelector<HTMLButtonElement>(`table ${kind}`) ?? Promise.reject(new Error(kind)));
          await act(async () => {
            fireEvent.click(button);
          });
        }
      }
    }, SWEEP);
  }
});

describe("a shared link", () => {
  it("names the comparison and keeps the language, nothing else", () => {
    expect(shareLink(token(4), "https://h.test/apps/x/?lang=sv&foo=1#/compare?codes=101")).toBe(`https://h.test/apps/x/?comparison=${token(4)}&lang=sv`);
    expect(shareLink(token(4), "https://h.test/apps/x/")).toBe(`https://h.test/apps/x/?comparison=${token(4)}`);
  });
});

describe("in Swedish", () => {
  it("speaks Swedish in the header and on the page", async () => {
    server.who = VIEWER;
    show("sv");
    expect(await screen.findByLabelText("Roll: Läsare")).toBeInTheDocument();
    expect(await screen.findByText("Du har inte sparat någon jämförelse ännu.")).toBeInTheDocument();
  });
});

describe("when the server cannot answer", () => {
  it("says the changes could not be read, and the page opens on the largest districts without the pin", async () => {
    server.who = VIEWER;
    server.changes = vi.fn(async () => {
      throw new Error("the database gave no answer");
    });
    server.pinned = vi.fn(async () => {
      throw new Error("the database gave no answer");
    });
    show();
    expect(await compared()).toHaveLength(2);
    expect(screen.getByTestId("pinned")).toHaveTextContent("Nothing is pinned");
    await open("Changes");
    expect(await screen.findByText("The changes could not be read: the database gave no answer")).toBeInTheDocument();
  });

  it("shows a change whose moment it cannot read as the server wrote it", async () => {
    server.who = VIEWER;
    server.log.push({ id: 1, at: "soon", actorName: VIEWER.name, action: "insert", target: "comparisons/1", summary: "kept", owner: VIEWER.id });
    show();
    await open("Changes");
    expect(await screen.findByRole("cell", { name: "soon" })).toBeInTheDocument();
  });
});
