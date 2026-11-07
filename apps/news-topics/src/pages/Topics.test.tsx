import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "../App";
import type { AnalysisOutput as TopicsOutput, TopicsHookResult as UseTopicsResult } from "../topics";
import { ServerContext } from "../server";
import type { Server } from "../server";
import { memory } from "../testing/server";

const NO_WEEKS: Server = memory();

type Click = (params: { name?: string }) => void;
const charts: Array<{ element: HTMLElement; setOption: ReturnType<typeof vi.fn>; click?: Click }> = [];
vi.mock("echarts", () => ({
  init: vi.fn((element: HTMLElement) => {
    const chart: (typeof charts)[number] = { element, setOption: vi.fn() };
    const api = {
      setOption: chart.setOption,
      resize: vi.fn(),
      dispose: vi.fn(),
      on: vi.fn((event: string, handler: Click) => {
        if (event === "click") chart.click = handler;
      }),
    };
    charts.push(chart);
    return api;
  }),
}));

const FIXED_TOPICS: TopicsOutput = {
  topics: [
    {
      id: 0,
      keywords: [
        { term: "school", weight: 0.8 },
        { term: "education", weight: 0.6 },
        { term: "pupils", weight: 0.5 },
      ],
      articles: [
        "urn:ngsi-ld:Article:hel.fi:helsinki:news-1",
        "urn:ngsi-ld:Article:hel.fi:helsinki:news-2",
      ],
      share: 0.6,
    },
    {
      id: 1,
      keywords: [
        { term: "traffic", weight: 0.7 },
        { term: "tram", weight: 0.5 },
        { term: "construction", weight: 0.4 },
      ],
      articles: [
        "urn:ngsi-ld:Article:hel.fi:helsinki:news-3",
      ],
      share: 0.4,
    },
  ],
  weeks: [
    {
      week: "2026-W40",
      shares: [0.5, 0.5],
    },
    {
      week: "2026-W41",
      shares: [0.7, 0.3],
    },
  ],
  unassigned: [],
};

let mockTopicsState: UseTopicsResult = {
  status: "ready",
  result: FIXED_TOPICS,
  error: null,
};

// The worker and its module are the WASM tests' (src/wasm.test.ts); here the page is given an answer.
vi.mock("../topics", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../topics")>()),
  useTopics: vi.fn(() => mockTopicsState),
}));

const ARTICLES: Row[] = [
  {
    id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-1",
    type: "Article",
    name: "New school opens in Kalasatama",
    description: "Modern learning spaces for 700 pupils in Kalasatama.",
    url: "https://www.hel.fi/en/news/new-school-kalasatama",
    datePublished: "2026-10-05T08:00:00Z",
    source: "https://www.hel.fi/en/news/rss",
  },
  {
    id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-2",
    type: "Article",
    name: "Education budget approved for next year",
    description: "City council approved funding for digital learning tools.",
    url: "https://www.hel.fi/en/news/education-budget",
    datePublished: "2026-10-06T10:00:00Z",
    source: "https://www.hel.fi/en/news/rss",
  },
  {
    id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-3",
    type: "Article",
    name: "Tram line extension starts construction",
    description: "Work on Crown Bridges tram connection advances to Hakaniemi.",
    url: "https://www.hel.fi/en/news/tram-construction",
    datePublished: "2026-10-12T09:00:00Z",
    source: "https://www.hel.fi/en/news/rss",
  },
];

const READ = {
  permissions: [{ resource: { type: "Article" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function renderTopics(entities: Row[] = ARTICLES, refuse?: () => { status: number; body: unknown } | null) {
  // `portal`: where the entity panel links an article (SDK-40); shown, never followed.
  const client = stubClient({ entities, access: READ, refuse }, { appName: "news-topics", portal: "https://portal.test/projects/helsinki" });
  render(
    <JcProvider client={client}>
      <ServerContext.Provider value={NO_WEEKS}>
        <App />
      </ServerContext.Provider>
    </JcProvider>,
  );
  return client;
}

describe("the topics page", () => {
  beforeEach(() => {
    charts.length = 0;
    window.history.replaceState(null, "", "/");
    mockTopicsState = {
      status: "ready",
      result: FIXED_TOPICS,
      error: null,
    };
  });

  afterEach(() => {
    window.history.replaceState(null, "", "/");
  });

  it("renders the first screen without a click: topics list, chart, articles of selected topic, and text alternative table", async () => {
    renderTopics(ARTICLES);
    await screen.findByRole("list", { name: /topics of the period/i });

    expect(screen.getByRole("heading", { level: 1, name: "Helsinki news topics" })).toBeInTheDocument();

    expect(screen.getByRole("region", { name: /topics in the news/i })).toBeInTheDocument();
    const topicsRegion = screen.getByRole("list", { name: /topics of the period/i });
    expect(within(topicsRegion).getByText(/school/i)).toBeInTheDocument();
    expect(within(topicsRegion).getByText(/traffic/i)).toBeInTheDocument();
    expect(within(topicsRegion).getByText(/60\s*%/)).toBeInTheDocument();
    expect(within(topicsRegion).getByText(/40\s*%/)).toBeInTheDocument();

    const chartFigure = screen.getByRole("figure");
    expect(chartFigure).toBeInTheDocument();
    expect(within(chartFigure).getByRole("img")).toBeInTheDocument();

    const altTable = screen.getByRole("table", { name: /topic share/i });
    expect(altTable).toBeInTheDocument();
    expect(within(altTable).getByText("2026-W40")).toBeInTheDocument();
    expect(within(altTable).getByText("2026-W41")).toBeInTheDocument();

    const articlesList = screen.getByRole("list", { name: /articles of topic/i });
    expect(within(articlesList).getByRole("heading", { name: "New school opens in Kalasatama" })).toBeInTheDocument();
    expect(within(articlesList).getByRole("heading", { name: "Education budget approved for next year" })).toBeInTheDocument();
    expect(within(articlesList).queryByRole("heading", { name: "Tram line extension starts construction" })).toBeNull();
  });

  it("topic selection updates the hash and displays articles of the selected topic", async () => {
    renderTopics(ARTICLES);

    const topic2Card = await screen.findByRole("button", { name: /^Topic 2: traffic/i });
    fireEvent.click(topic2Card);

    expect(window.location.hash).toContain("topic=1");

    await waitFor(() => {
      const articlesList = screen.getByRole("list", { name: /articles of topic/i });
      expect(within(articlesList).getByRole("heading", { name: "Tram line extension starts construction" })).toBeInTheDocument();
      expect(within(articlesList).queryByRole("heading", { name: "New school opens in Kalasatama" })).toBeNull();
    });
  });

  it("respects initial topic selection from the URL hash", async () => {
    window.history.replaceState(null, "", "/#topics?topic=1");
    renderTopics(ARTICLES);

    await waitFor(() => {
      const articlesList = screen.getByRole("list", { name: /articles of topic/i });
      expect(within(articlesList).getByRole("heading", { name: "Tram line extension starts construction" })).toBeInTheDocument();
      expect(within(articlesList).queryByRole("heading", { name: "New school opens in Kalasatama" })).toBeNull();
    });
  });

  it("renders empty state when there are no news articles in the period", async () => {
    mockTopicsState = {
      status: "ready",
      result: { topics: [], weeks: [], unassigned: [] },
      error: null,
    };
    renderTopics([]);

    await waitFor(() => {
      expect(screen.getByText(/no news in this period/i)).toBeInTheDocument();
    });
  });

  it("renders error alert with retry button when endpoint fails", async () => {
    let fail = true;
    renderTopics(ARTICLES, () => (fail ? { status: 503, body: { title: "Service Unavailable" } } : null));

    const alert = await screen.findByRole("alert");
    expect(alert).toBeInTheDocument();

    fail = false;
    const retryBtn = within(alert).getByRole("button", { name: /retry/i });
    fireEvent.click(retryBtn);

    await waitFor(() => {
      expect(screen.queryByRole("alert")).toBeNull();
    });
  });

  it("renders error alert when worker fails", async () => {
    mockTopicsState = {
      status: "error",
      result: null,
      error: new Error("WASM clustering worker crashed"),
    };
    renderTopics(ARTICLES);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("WASM clustering worker crashed");
  });

  it("renders Finnish UI strings when URL has ?lang=fi", async () => {
    window.history.replaceState(null, "", "/?lang=fi");
    renderTopics(ARTICLES);

    expect(screen.getByRole("heading", { level: 1, name: "Helsingin uutisaiheet" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: /uutisaiheet/i })).toBeInTheDocument();
  });

  it("renders Finnish empty state when there are no articles", async () => {
    window.history.replaceState(null, "", "/?lang=fi");
    mockTopicsState = {
      status: "ready",
      result: { topics: [], weeks: [], unassigned: [] },
      error: null,
    };
    renderTopics([]);

    await waitFor(() => {
      expect(screen.getByText(/ei uutisia tällä ajanjaksolla/i)).toBeInTheDocument();
    });
  });

  it("satisfies accessibility role checks and proper links", async () => {
    renderTopics(ARTICLES);
    await screen.findByRole("list", { name: /topics of the period/i });

    expect(screen.getByRole("search", { name: /filter/i })).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: /search/i })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: /topics of the period/i })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: /topic share/i })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: /articles of topic/i })).toBeInTheDocument();

    const link = screen.getByRole("link", { name: "Read on hel.fi: New school opens in Kalasatama" });
    expect(link).toHaveAttribute("href", "https://www.hel.fi/en/news/new-school-kalasatama");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
  });

  it("links an article only to a web address, never to what else a feed entry may hold", async () => {
    const hostile = { ...ARTICLES[0], url: "javascript:alert(1)" };
    renderTopics([hostile, ARTICLES[1], ARTICLES[2]]);
    await screen.findByRole("list", { name: /topics of the period/i });
    const articles = screen.getByRole("list", { name: /articles of topic/i });
    expect(within(articles).getByRole("heading", { name: "New school opens in Kalasatama" })).toBeInTheDocument();
    expect(within(articles).queryByRole("link", { name: /New school opens in Kalasatama/ })).toBeNull();
    expect(within(articles).getByRole("link", { name: /Education budget approved/ })).toHaveAttribute("href", "https://www.hel.fi/en/news/education-budget");
  });

  // SDK-40, T-3401: an article opens in the shell's entity panel, linked to the Portal, without Edit.
  it("opens each article of a topic in the entity panel", async () => {
    renderTopics(ARTICLES);
    const articles = await screen.findByRole("list", { name: /articles of topic/i });
    for (const name of ["New school opens in Kalasatama", "Education budget approved for next year"]) {
      fireEvent.click(within(articles).getByRole("button", { name }));
      const panel = await screen.findByRole("dialog", { name });
      const portal = await within(panel).findByRole("link", { name: "Open in the Portal" });
      fireEvent.click(portal);
      expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
      fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    }
    fireEvent.click(screen.getByRole("button", { name: "Topic 2: traffic, tram, construction" }));
    const traffic = screen.getByRole("list", { name: /articles of topic/i });
    fireEvent.click(await within(traffic).findByRole("button", { name: "Tram line extension starts construction" }));
    expect(await screen.findByRole("dialog", { name: "Tram line extension starts construction" })).toBeInTheDocument();
    fireEvent.click(within(traffic).getByRole("link", { name: "Read on hel.fi: Tram line extension starts construction" }));
  });

  // T-3373: every control answers: the period, the number of topics and the search narrow what is
  // analysed and stay in the address; the table opens and closes; a topic is named by its keywords.
  it("keeps the period, the number of topics, the search and the topic in the address", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-10-15T12:00:00Z"));
    renderTopics(ARTICLES);
    await screen.findByRole("list", { name: /topics of the period/i });
    const first = screen.getByRole("button", { name: "Topic 1: school, education, pupils" });
    expect(first).toHaveAttribute("aria-pressed", "true");
    expect(first).toHaveAccessibleDescription(/60\s*%.*2 articles/);
    fireEvent.click(first);
    expect(window.location.hash).toBe("#topics?topic=0");

    fireEvent.change(screen.getByRole("combobox", { name: "Period" }), { target: { value: "4" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Number of topics" }), { target: { value: "3" } });
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "  tram  " } });
    expect(new URLSearchParams(window.location.hash.slice(window.location.hash.indexOf("?") + 1)).toString()).toBe("weeks=4&k=3&topic=0&q=tram");

    const toggle = screen.getByRole("button", { name: "Show the numbers as a table" });
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: /hide the table/i }));
    for (const link of screen.getAllByRole("link", { name: /^Read on hel\.fi/ })) fireEvent.click(link);
    vi.useRealTimers();
  });

  it("reads the address it was given, and falls back for what it cannot read", async () => {
    window.history.replaceState(null, "", "/#topics?weeks=99&k=42&topic=x&q=school");
    renderTopics(ARTICLES);
    await screen.findByRole("list", { name: /topics of the period/i });
    expect(screen.getByRole("combobox", { name: "Period" })).toHaveValue("all");
    expect(screen.getByRole("combobox", { name: "Number of topics" })).toHaveValue("5");
    expect(screen.getByRole("searchbox", { name: "Search" })).toHaveValue("school");
    // The search leaves only what it finds, so the empty answer of a narrowed list is the page's.
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "nothing like it" } });
    expect(await screen.findByText(/no news in this period/i)).toBeInTheDocument();
    // Back from somewhere else in the address.
    window.history.replaceState(null, "", "/#topics?weeks=8");
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Period" })).toHaveValue("8"));
  });

  it("answers every control in Finnish", async () => {
    window.history.replaceState(null, "", "/?lang=fi");
    renderTopics(ARTICLES);
    await screen.findByRole("list", { name: /ajanjakson aiheet/i });
    fireEvent.change(screen.getByRole("combobox", { name: "Ajanjakso" }), { target: { value: "12" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Aiheiden määrä" }), { target: { value: "4" } });
    fireEvent.change(screen.getByRole("searchbox", { name: "Hae" }), { target: { value: "" } });
    expect(window.location.hash).toBe("#topics?weeks=12&k=4");
    for (const topic of ["Aihe 1: school, education, pupils", "Aihe 2: traffic, tram, construction"]) fireEvent.click(screen.getByRole("button", { name: topic }));
    fireEvent.click(screen.getByRole("button", { name: "Aihe 1: school, education, pupils" }));
    fireEvent.click(screen.getByRole("button", { name: "Näytä luvut taulukkona" }));
    fireEvent.click(screen.getByRole("button", { name: "Piilota taulukko" }));
    for (const link of screen.getAllByRole("link", { name: /^Lue hel\.fi:ssä/ })) fireEvent.click(link);
  });

  // What the analysis or the feed may hand the page that it still shows.
  it("shows an undated and an unnamed article, a topic with no keyword and a week with no share", async () => {
    const undated: Row = { id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-9", type: "Article" };
    mockTopicsState = {
      status: "ready",
      result: {
        topics: [
          { id: 0, keywords: [], articles: [undated.id, ARTICLES[0].id], share: 1 },
          { id: 1, keywords: [{ term: "tram", weight: 1 }], articles: ["urn:ngsi-ld:Article:hel.fi:helsinki:gone"], share: 0 },
        ],
        weeks: [{ week: "2026-W41", shares: [1] }],
        unassigned: [],
      },
      error: null,
    };
    window.history.replaceState(null, "", "/#topics?topic=7&weeks=4");
    renderTopics([...ARTICLES, undated]);
    const articles = await screen.findByRole("list", { name: /articles of topic/i });
    // The dated article first, the undated one after it, named by its id and said to have no date.
    const titles = within(articles).getAllByRole("heading").map((heading) => heading.textContent);
    expect(titles).toEqual(["New school opens in Kalasatama", undated.id]);
    expect(within(articles).getByText(/no date/i)).toBeInTheDocument();
    // The topic names arrive from the App's server after the first draw, and the chart draws again.
    const option = await waitFor(() => {
      const last = charts.at(-1)?.setOption.mock.calls.at(-1)?.[0];
      expect(last).toBeDefined();
      return last as { series: Array<{ name: string; data: number[] }>; tooltip: { valueFormatter: (value: unknown) => string } };
    });
    expect(option.series.map((series) => series.name)).toEqual(["Topic 1", "Topic 2: tram"]);
    expect(option.series[1].data).toEqual([0]);
    expect(option.tooltip.valueFormatter(12.345)).toBe("12.3%");
    expect(option.tooltip.valueFormatter("-")).toBe("-");
    fireEvent.click(screen.getByRole("button", { name: "Show the numbers as a table" }));
    expect(within(screen.getByRole("table", { name: /topic share/i })).getAllByText(/0\s*%/).length).toBeGreaterThan(0);
    // A topic none of whose articles the page holds says so; its button names it by its keywords.
    fireEvent.click(screen.getByRole("button", { name: "Topic 2: tram" }));
    expect(await screen.findByText(/no news in this period/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Topic 1" }));
    fireEvent.click(within(screen.getByRole("list", { name: /articles of topic/i })).getByRole("button", { name: undated.id }));
    fireEvent.click(within(screen.getByRole("list", { name: /articles of topic/i })).getByRole("button", { name: "New school opens in Kalasatama" }));
    fireEvent.click(screen.getByRole("link", { name: "Read on hel.fi: New school opens in Kalasatama" }));
    // Back to every default: the address keeps no parameter.
    fireEvent.change(screen.getByRole("combobox", { name: "Period" }), { target: { value: "all" } });
    fireEvent.click(screen.getByRole("button", { name: "Hide the table" }));
  });

  it("says it is analysing while the first answer is on its way, and has no weekly chart for an answer with no week", async () => {
    mockTopicsState = { status: "loading", result: null, error: null };
    renderTopics(ARTICLES);
    expect(await screen.findByRole("status")).toBeInTheDocument();
    cleanup();
    mockTopicsState = { status: "ready", result: { ...FIXED_TOPICS, weeks: [] }, error: null };
    renderTopics(ARTICLES);
    const chart = (await screen.findByText("Topic share per week", { selector: "figcaption" })).closest("figure") as HTMLElement;
    expect(within(chart).getByText(/no news in this period/i)).toBeInTheDocument();
    for (const name of ["Topic 1: school, education, pupils", "Topic 2: traffic, tram, construction"]) fireEvent.click(screen.getByRole("button", { name }));
    for (const name of ["New school opens in Kalasatama", "Education budget approved for next year"]) {
      fireEvent.click(screen.getByRole("button", { name: "Topic 1: school, education, pupils" }));
      fireEvent.click(within(screen.getByRole("list", { name: /articles of topic/i })).getByRole("button", { name }));
      fireEvent.click(screen.getByRole("link", { name: `Read on hel.fi: ${name}` }));
    }
    fireEvent.click(screen.getByRole("button", { name: "Show the numbers as a table" }));
  });
});
