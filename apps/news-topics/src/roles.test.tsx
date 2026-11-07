/**
 * T-3828: who reads, and what they change. A signed-in reader follows topics by their top keyword
 * and finds them again in any period; a steward names a topic, renames it, merges two by giving
 * them one name, and takes a name back, which everyone reads. Each change shows at once and is
 * taken back when the server refuses; a closed control says why; the Changes page shows the history.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import type { AnalysisOutput } from "./topics";
import type { Me } from "./server";
import { ServerContext } from "./server";
import { memory, STEWARD, VIEWER } from "./testing/server";
import type { Memory } from "./testing/server";
import { sweep } from "./testing/sweep";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

vi.mock("echarts", () => ({ init: () => ({ setOption: () => undefined, resize: () => undefined, dispose: () => undefined, on: () => undefined }) }));

const article = (n: number, name: string): Row => ({
  id: `urn:ngsi-ld:Article:hel.fi:helsinki:news-${n}`,
  type: "Article",
  name,
  datePublished: `2026-10-0${n}T08:00:00Z`,
});
const ARTICLES = [article(1, "New school opens"), article(2, "Tram line works"), article(3, "Library reading room")];
const TOPICS: AnalysisOutput = {
  topics: [
    { id: 0, keywords: [{ term: "school", weight: 0.8 }, { term: "pupils", weight: 0.5 }], articles: [ARTICLES[0].id], share: 0.4 },
    { id: 1, keywords: [{ term: "tram", weight: 0.7 }, { term: "works", weight: 0.4 }], articles: [ARTICLES[1].id], share: 0.35 },
    { id: 2, keywords: [{ term: "library", weight: 0.6 }], articles: [ARTICLES[2].id], share: 0.25 },
  ],
  weeks: [{ week: "2026-W41", shares: [0.4, 0.35, 0.25] }],
  unassigned: [],
};
vi.mock("./topics", async (original) => ({
  ...(await original<typeof import("./topics")>()),
  useTopics: () => ({ status: "ready", result: TOPICS, error: null }),
}));

const READ = {
  permissions: [{ resource: { type: "Article" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function show(who: Me | null, language = "en", before: (server: Memory) => void = () => undefined): Memory {
  window.history.replaceState(null, "", `/?lang=${language}#topics`);
  const server = memory(who);
  before(server);
  render(
    <JcProvider client={stubClient({ entities: ARTICLES, access: READ }, { appName: "news-topics" })}>
      <ServerContext.Provider value={server}>
        <App />
      </ServerContext.Provider>
    </JcProvider>,
  );
  return server;
}

const main = () => screen.getByRole("main");
const page = async (name: string) => fireEvent.click(await screen.findByRole("button", { name }));
const said = (text: string | RegExp) => screen.findByText(text);
const topicsList = () => screen.getByRole("list", { name: "Topics of the period" });
const named = (sara = "Sara Steward") => ({ id: 50, term: "tram", name: "Trams", ownerName: sara });

afterEach(() => window.history.replaceState(null, "", "/"));

describe("a signed-out reader", () => {
  it("reads the topic names, changes nothing, and every closed control says why", async () => {
    const server = show(null, "en", (fresh) => fresh.given.push(named()));
    expect(await screen.findByLabelText("Role: Signed out")).toBeInTheDocument();
    expect(await within(topicsList()).findByText("Trams")).toBeInTheDocument();
    expect(screen.getByText("tram: Trams, by Sara Steward")).toBeInTheDocument();
    const follow = screen.getByRole("button", { name: "Follow school" });
    expect(follow).toBeDisabled();
    expect(follow).toHaveAttribute("title", "Sign in to follow topics.");
    fireEvent.click(screen.getByRole("button", { name: "New school opens" }));
    expect(await screen.findByRole("dialog", { name: "New school opens" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.getByRole("textbox", { name: "Name of the selected topic" })).toHaveAttribute("title", "Sign in as a steward to name and merge topics.");
    expect(screen.getByRole("button", { name: "Take back the name of tram" })).toBeDisabled();
    expect(screen.getAllByText("Sign in to follow topics.").length).toBeGreaterThan(0);
    await sweep(main());
    expect(server.followed).toHaveLength(0);
    expect(server.given).toHaveLength(1);
    await page("Changes");
    expect(await screen.findByText("Sign in to see the changes.")).toBeInTheDocument();
    await page("Topics");
    expect(await screen.findByRole("list", { name: "Topics of the period" })).toBeInTheDocument();
  }, SWEEP);

  it("is signed out when the server cannot say who reads, and says so when the names cannot be read", async () => {
    show(VIEWER, "en", (fresh) => {
      fresh.fail.me = "store down";
      fresh.fail.names = "store down";
    });
    expect(await screen.findByLabelText("Role: Signed out")).toBeInTheDocument();
    expect(await said("The topic names could not be read: store down")).toBeInTheDocument();
    expect(screen.queryByText("No topic has a name yet.")).toBeNull();
  });
});

describe("a viewer", () => {
  it("follows a topic, finds it among the period's topics, and lets it go; naming stays a steward's", async () => {
    const server = show(VIEWER);
    expect(await screen.findByLabelText("Role: Viewer")).toBeInTheDocument();
    expect(await said("You follow no topic yet.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Follow school" }));
    expect(await said("Following: school")).toBeInTheDocument();
    expect(server.followed).toMatchObject([{ term: "school", owner: "u-viewer" }]);
    expect(screen.getByRole("button", { name: "Follow school" })).toHaveAttribute("aria-pressed", "true");
    const followed = screen.getByRole("list", { name: "Followed topics" });
    expect(within(followed).getByText("in this period's topics")).toBeInTheDocument();

    const name = screen.getByRole("textbox", { name: "Name of the selected topic" });
    expect(name).toBeDisabled();
    expect(name).toHaveAttribute("title", "Only a steward names and merges topics.");

    fireEvent.click(within(followed).getByRole("button", { name: "Stop following school" }));
    expect(await said("No longer following: school")).toBeInTheDocument();
    expect(server.followed).toHaveLength(0);
  });

  it("sees a followed keyword that is not among the period's topics, a refused follow taken back and a refused stop put back", async () => {
    const server = show(VIEWER, "en", (fresh) => fresh.followed.push({ id: 40, term: "harbour", owner: "u-viewer" }));
    expect(await said("not among this period's topics")).toBeInTheDocument();
    server.fail.follow = "at most 30 keywords are followed; stop following one first";
    fireEvent.click(screen.getByRole("button", { name: "Follow tram" }));
    expect(await said("Not saved: at most 30 keywords are followed; stop following one first")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Follow tram" })).toHaveAttribute("aria-pressed", "false");

    server.fail.unfollow = "store down";
    fireEvent.click(screen.getByRole("button", { name: "Stop following harbour" }));
    expect(await said("Not saved: store down")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop following harbour" })).toBeInTheDocument();
  });

  it("says so when the followed topics cannot be read", async () => {
    show(VIEWER, "en", (fresh) => {
      fresh.fail.follows = "store down";
    });
    expect(await said("The followed topics could not be read: store down")).toBeInTheDocument();
    expect(screen.queryByText("You follow no topic yet.")).toBeNull();
  });

  it("uses every control in Finnish, and sees only their own changes", async () => {
    const server = show(VIEWER, "fi", (fresh) =>
      fresh.audit.push({ id: 99, at: "2026-10-11T09:00:00Z", actorId: "u-steward", actorName: "Sara Steward", action: "insert", target: "topic_names/1", summary: "named the topic of tram Trams" }),
    );
    expect(await screen.findByLabelText("Rooli: Katselija")).toBeInTheDocument();
    await said("Et seuraa vielä yhtään aihetta.");
    await sweep(main());
    // The sweep followed topics and let one go again, all as the viewer.
    expect(server.audit.filter((row) => row.actorId === "u-viewer").map((row) => row.action)).toEqual(expect.arrayContaining(["insert", "delete"]));
    // The sweep opened an article in the shell's panel, outside the page it sweeps.
    fireEvent.click(await screen.findByRole("button", { name: "Sulje" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Kieli" }), { target: { value: "fi" } });
    await page("Muutokset");
    expect(await screen.findByText("Omat muutoksesi, uusin ensin.")).toBeInTheDocument();
    expect(screen.queryByText(/Trams/)).toBeNull();
    await page("Aiheet");
    expect(await screen.findByText("Seuratut aiheet")).toBeInTheDocument();
  }, SWEEP);
});

describe("a steward", () => {
  it("names the selected topic, renames it, merges two topics into one, and takes a name back", async () => {
    const server = show(STEWARD);
    expect(await screen.findByLabelText("Role: Steward")).toBeInTheDocument();
    const name = await screen.findByRole("textbox", { name: "Name of the selected topic" });
    fireEvent.click(screen.getByRole("button", { name: "Name the topic" }));
    expect(await said("Give the topic a name (at most 80 characters).")).toBeInTheDocument();
    fireEvent.change(name, { target: { value: " Schools " } });
    fireEvent.click(screen.getByRole("button", { name: "Name the topic" }));
    expect(await said("Named: Schools")).toBeInTheDocument();
    expect(within(topicsList()).getByText("Schools")).toBeInTheDocument();
    expect(server.given).toMatchObject([{ term: "school", name: "Schools" }]);

    fireEvent.change(screen.getByRole("textbox", { name: "Name of the selected topic" }), { target: { value: "Schools and pupils" } });
    fireEvent.click(screen.getByRole("button", { name: "Name the topic" }));
    expect(await said("Named: Schools and pupils")).toBeInTheDocument();
    expect(server.given).toHaveLength(1);

    // Merging with an unnamed topic names both with that topic's keywords.
    fireEvent.change(screen.getByRole("combobox", { name: "Merge the selected topic with" }), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Merge" }));
    expect(await said("Merged into library")).toBeInTheDocument();
    await waitFor(() => expect(within(topicsList()).getAllByRole("listitem")).toHaveLength(2));
    fireEvent.click(screen.getByRole("button", { name: "Topic 1: school, pupils, library" }));
    expect(screen.getByRole("button", { name: "Topic 1: school, pupils, library" })).toHaveAttribute("aria-pressed", "true");
    expect(server.given.map((entry) => [entry.term, entry.name])).toEqual([
      ["library", "library"],
      ["school", "library"],
    ]);

    fireEvent.click(screen.getByRole("button", { name: "Take back the name of school" }));
    expect(await said("Name taken back: library")).toBeInTheDocument();
    await waitFor(() => expect(within(topicsList()).getAllByRole("listitem")).toHaveLength(3));
  });

  it("merges into a named topic under its name, and sees a refused name or take-back undone", async () => {
    const server = show(STEWARD, "en", (fresh) => fresh.given.push(named()));
    const merge = await screen.findByRole("combobox", { name: "Merge the selected topic with" });
    fireEvent.change(merge, { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "Merge" }));
    expect(await said("Merged into Trams")).toBeInTheDocument();
    expect(server.given.map((entry) => entry.term)).toEqual(["tram", "school"]);
    fireEvent.click(await screen.findByRole("button", { name: "Topic 1: school, pupils, tram, works" }));

    server.fail.name = "store down";
    fireEvent.change(screen.getByRole("textbox", { name: "Name of the selected topic" }), { target: { value: "Rails" } });
    fireEvent.click(screen.getByRole("button", { name: "Name the topic" }));
    expect(await said("Not saved: store down")).toBeInTheDocument();
    expect(screen.getByText("school: Trams, by Sara Steward")).toBeInTheDocument();

    server.fail.unname = "store down";
    fireEvent.click(screen.getByRole("button", { name: "Take back the name of tram" }));
    expect(await said("Not saved: store down")).toBeInTheDocument();
    expect(screen.getByText("tram: Trams, by Sara Steward")).toBeInTheDocument();
  });

  it("a name refused for a topic named before puts the old name back", async () => {
    const server = show(STEWARD, "en", (fresh) => fresh.given.push({ ...named(), term: "school", name: "Schools" }));
    const name = await screen.findByRole("textbox", { name: "Name of the selected topic" });
    server.fail.name = "store down";
    fireEvent.change(name, { target: { value: "Pupils" } });
    fireEvent.click(screen.getByRole("button", { name: "Name the topic" }));
    expect(await said("Not saved: store down")).toBeInTheDocument();
    expect(within(topicsList()).getByText("Schools")).toBeInTheDocument();
  });

  it("uses every control in English", async () => {
    show(STEWARD, "en", (fresh) => fresh.given.push(named(), { id: 51, term: "library", name: "Libraries", ownerName: "Sara Steward" }));
    expect(await screen.findByLabelText("Role: Steward")).toBeInTheDocument();
    await said("You follow no topic yet.");
    await sweep(main());
    expect(screen.getByRole("main")).toBeInTheDocument();
  }, SWEEP);

  it("uses every control in Swedish, and sees everyone's changes", async () => {
    const server = show(STEWARD, "sv", (fresh) =>
      fresh.audit.push({ id: 99, at: "2026-10-11T09:00:00Z", actorId: "u-viewer", actorName: "Vera Viewer", action: "insert", target: "follows/1", summary: "followed harbour" }),
    );
    expect(await screen.findByLabelText("Roll: Förvaltare")).toBeInTheDocument();
    await said("Du följer inget ämne ännu.");
    await sweep(main());
    await page("Ändringar");
    expect(await screen.findByText("Allas ändringar, nyaste först.")).toBeInTheDocument();
    expect(await screen.findByText(/followed harbour/)).toBeInTheDocument();
    server.fail.changes = "store down";
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Uppdatera" }));
    });
    expect(await said("Ändringarna kunde inte läsas: store down")).toBeInTheDocument();
    await page("Ämnen");
    expect(await screen.findByText("Följda ämnen")).toBeInTheDocument();
  }, SWEEP);
});

describe("the language", () => {
  it("switches in the shell's header and stays in the address", async () => {
    show(null);
    fireEvent.change(await screen.findByRole("combobox", { name: "Language" }), { target: { value: "sv" } });
    expect(await screen.findByRole("heading", { level: 1, name: "Helsingfors nyhetsämnen" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("lang")).toBe("sv");
    expect(document.documentElement.lang).toBe("sv");
  });
});
