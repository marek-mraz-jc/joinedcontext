import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { mergeNamed, toInput, topTerm, useTopics } from "./topics";
import type { AnalysisOutput } from "./topics";

describe("toInput", () => {
  it("converts SDK rows with complete attributes to article inputs", () => {
    const rows: Row[] = [
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-1",
        type: "Article",
        name: "School opening in Kalasatama",
        description: "Modern facilities for children",
        datePublished: "2026-10-05T08:00:00Z",
        url: "https://www.hel.fi/en/news/school-opening",
      },
    ];

    const input = toInput(rows, 5);

    expect(input.k).toBe(5);
    expect(input.seed).toBe(1);
    expect(input.articles).toEqual([
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-1",
        title: "School opening in Kalasatama",
        summary: "Modern facilities for children",
        published: "2026-10-05T08:00:00Z",
      },
    ]);
  });

  it("omits summary when description is missing, undefined, null, or empty", () => {
    const rows: Row[] = [
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-no-desc",
        type: "Article",
        name: "Brief announcement",
        datePublished: "2026-10-06T10:00:00Z",
      },
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-null-desc",
        type: "Article",
        name: "Another notice",
        description: null,
        datePublished: "2026-10-06T11:00:00Z",
      },
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-empty-desc",
        type: "Article",
        name: "Notice with blank summary",
        description: "   ",
        datePublished: "2026-10-06T12:00:00Z",
      },
    ];

    const input = toInput(rows, 3);

    expect(input.articles[0].summary).toBeUndefined();
    expect(input.articles[1].summary).toBeUndefined();
    expect(input.articles[2].summary).toBeUndefined();
    expect(input.articles[0].title).toBe("Brief announcement");
    expect(input.articles[0].published).toBe("2026-10-06T10:00:00Z");
  });

  it("omits published when datePublished is missing, undefined, null, or empty", () => {
    const rows: Row[] = [
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-no-date",
        type: "Article",
        name: "Undated story",
        description: "Some summary",
      },
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-null-date",
        type: "Article",
        name: "Another undated story",
        datePublished: null,
      },
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-empty-date",
        type: "Article",
        name: "Story with blank date",
        datePublished: "   ",
      },
    ];

    const input = toInput(rows, 4);

    expect(input.articles[0].published).toBeUndefined();
    expect(input.articles[1].published).toBeUndefined();
    expect(input.articles[2].published).toBeUndefined();
    expect(input.articles[0].title).toBe("Undated story");
    expect(input.articles[0].summary).toBe("Some summary");
  });

  it("handles rows with both description and datePublished missing", () => {
    const rows: Row[] = [
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-minimal",
        type: "Article",
        name: "Only title",
      },
    ];

    const input = toInput(rows, 2);

    expect(input.articles).toEqual([
      {
        id: "urn:ngsi-ld:Article:hel.fi:helsinki:news-minimal",
        title: "Only title",
      },
    ]);
  });

  it("supports custom seed and handles empty rows array", () => {
    const emptyInput = toInput([], 5, 42);
    expect(emptyInput).toEqual({
      articles: [],
      k: 5,
      seed: 42,
    });
  });
});

// The page's clustering runs off its thread: one request at a time, a stale answer dropped, and
// every way the worker can fail said in words.
describe("useTopics", () => {
  const started: FakeWorker[] = [];
  class FakeWorker {
    onmessage: ((event: { data: unknown }) => void) | null = null;
    onerror: ((event: { message: string }) => void) | null = null;
    posted: Array<{ id: number }> = [];
    terminated = false;
    constructor() {
      started.push(this);
    }
    postMessage(message: { id: number }) {
      this.posted.push(message);
    }
    terminate() {
      this.terminated = true;
    }
  }
  const ROW: Row = { id: "urn:ngsi-ld:Article:hel.fi:helsinki:1", type: "Article", name: "Tram line opens" };
  const ANSWER: AnalysisOutput = { topics: [], weeks: [], unassigned: ["urn:ngsi-ld:Article:hel.fi:helsinki:1"] };

  beforeEach(() => {
    started.length = 0;
    vi.stubGlobal("Worker", FakeWorker);
  });
  afterEach(() => vi.unstubAllGlobals());

  it("answers the latest request only, and stops the worker with the page", () => {
    const view = renderHook(({ k }) => useTopics([ROW], k), { initialProps: { k: 3 } });
    const worker = started[0];
    expect(view.result.current.status).toBe("loading");
    view.rerender({ k: 4 });
    const [old, latest] = worker.posted;
    act(() => worker.onmessage?.({ data: { id: old.id, output: ANSWER } }));
    act(() => worker.onmessage?.({ data: null }));
    expect(view.result.current.status).toBe("loading");
    act(() => worker.onmessage?.({ data: { id: latest.id, output: ANSWER } }));
    expect(view.result.current).toEqual({ status: "ready", result: ANSWER, error: null });
    // A message with neither an answer nor an error changes nothing.
    act(() => worker.onmessage?.({ data: { id: latest.id } }));
    expect(view.result.current.status).toBe("ready");
    view.unmount();
    expect(worker.terminated).toBe(true);
  });

  it("says what the worker or the module could not do", () => {
    const view = renderHook(() => useTopics([ROW]));
    const worker = started[0];
    const id = worker.posted[0].id;
    act(() => worker.onmessage?.({ data: { id, error: "the module stopped" } }));
    expect(view.result.current.error?.message).toBe("the module stopped");
    act(() => worker.onmessage?.({ data: { id, output: { error: "Invalid input JSON" } } }));
    expect(view.result.current).toMatchObject({ status: "error", result: null });
    expect(view.result.current.error?.message).toBe("Invalid input JSON");
    act(() => worker.onerror?.({ message: "" }));
    expect(view.result.current.error?.message).toBe("Worker error");
    act(() => worker.onerror?.({ message: "out of memory" }));
    expect(view.result.current.error?.message).toBe("out of memory");
  });

  it("answers no article with no topic, without asking the worker", () => {
    const view = renderHook(() => useTopics([]));
    expect(view.result.current).toEqual({ status: "ready", result: { topics: [], weeks: [], unassigned: [] }, error: null });
    expect(started[0].posted).toEqual([]);
  });

  it("says so when no worker can be started", () => {
    vi.stubGlobal(
      "Worker",
      class {
        constructor() {
          throw new Error("Workers are blocked");
        }
      },
    );
    const view = renderHook(() => useTopics([ROW]));
    expect(view.result.current).toMatchObject({ status: "error" });
    expect(view.result.current.error?.message).toBe("Workers are blocked");
  });

  it("says so in words when the browser refuses a worker with no Error at all", () => {
    vi.stubGlobal(
      "Worker",
      class {
        constructor() {
          throw "refused";
        }
      },
    );
    const view = renderHook(() => useTopics([{ id: "urn:ngsi-ld:Article:hel.fi:helsinki:2", type: "Article" }]));
    expect(view.result.current.error?.message).toBe("refused");
  });
});

describe("mergeNamed", () => {
  const topic = (id: number, terms: string[], share: number, articles: string[]) => ({
    id,
    keywords: terms.map((term, i) => ({ term, weight: 1 - i / 10 })),
    articles,
    share,
  });
  const result = {
    topics: [topic(0, ["kirjasto", "laina"], 0.5, ["a"]), topic(1, ["raitiotie"], 0.3, ["b"]), topic(2, ["lukusali", "kirjasto"], 0.2, ["c"])],
    weeks: [
      { week: "2026-W40", shares: [0.4, 0.4, 0.2] },
      { week: "2026-W41", shares: [0.6] },
    ],
    unassigned: ["z"],
  };

  it("joins the topics a steward gave one name, and leaves the others as they are", () => {
    const names: Record<string, string> = { kirjasto: "Libraries", lukusali: "Libraries", raitiotie: "Trams" };
    const merged = mergeNamed(result, (term) => names[term]);
    expect(merged.topics).toHaveLength(2);
    expect(merged.topics[0]).toEqual({
      id: 0,
      keywords: [
        { term: "kirjasto", weight: 1 },
        { term: "laina", weight: 0.9 },
        { term: "lukusali", weight: 1 },
      ],
      articles: ["a", "c"],
      share: 0.7,
    });
    expect(merged.topics[1]).toBe(result.topics[1]);
    expect(merged.weeks[0].shares[0]).toBeCloseTo(0.6);
    // A week that held none of a merged topic's articles counts it as nothing.
    expect(merged.weeks[1].shares[0]).toBeCloseTo(0.6);
    expect(merged.unassigned).toEqual(["z"]);
  });

  it("changes nothing when no topic has a name, and a topic with no keyword has no top keyword", () => {
    const merged = mergeNamed(result, () => undefined);
    expect(merged.topics).toEqual(result.topics);
    expect(merged.weeks).toEqual(result.weeks);
    expect(topTerm(topic(3, [], 0, []))).toBe("");
  });
});
