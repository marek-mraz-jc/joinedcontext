import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAX_H1,
  MAX_LABEL,
  MAX_PAGES,
  MAX_TEXT,
  navItems,
  observe,
  resetObserver,
  snapshot,
  startObserver,
  versionOf,
  waitForIdle,
} from "../src/sdk/observe";
import { activity, bridgeTransport, MAX_FAILED_PATH, resetActivity } from "../src/sdk/transport";

/** A frame's window: a parent to post to, and the location the preview URL gave the document. */
function frameWindow(search = "?v=3", hash = "") {
  const parent = { postMessage: vi.fn() };
  const location = { search, hash };
  const listeners: ((event: MessageEvent) => void)[] = [];
  const addEventListener = (type: string, listener: (event: MessageEvent) => void) => {
    if (type === "message") listeners.push(listener);
  };
  /** A message to the frame, from the host page unless `source` says otherwise. */
  const send = (data: unknown, source: unknown = parent) => {
    for (const listener of listeners) listener({ data, source } as MessageEvent);
  };
  return { win: { parent, location, addEventListener } as unknown as Window, parent, location, send };
}

const FAST = { quietMs: 20, maxWaitMs: 500, pageQuietMs: 10, pageMaxWaitMs: 200, pollMs: 5 };

beforeEach(() => {
  resetActivity();
  resetObserver();
  document.body.innerHTML = "";
  document.title = "";
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("waitForIdle", () => {
  it("waits until no request has been pending for the quiet time", async () => {
    activity.pending = 1;
    const started = Date.now();
    setTimeout(() => {
      activity.pending = 0;
      activity.lastChange = Date.now();
    }, 60);
    await expect(waitForIdle(40, 2000, 5)).resolves.toBe(true);
    expect(Date.now() - started).toBeGreaterThanOrEqual(95);
  });

  it("gives up at the ceiling while a request stays pending", async () => {
    activity.pending = 1;
    await expect(waitForIdle(10, 60, 5)).resolves.toBe(false);
  });

  it("counts a bridge request as pending until the Portal answers, and keeps an error status", async () => {
    const target = { postMessage: vi.fn() } as unknown as Window;
    const transport = bridgeTransport({ target, self: window, timeoutMs: 1000 });
    const answer = transport({ method: "POST", path: `/functions/${"x".repeat(300)}` });
    expect(activity.pending).toBe(1);
    const { id } = (target.postMessage as ReturnType<typeof vi.fn>).mock.calls[0][0] as { id: number };
    window.dispatchEvent(new MessageEvent("message", { source: target, data: { kind: "jc-response", id, status: 500, body: null } }));
    await answer;
    expect(activity.pending).toBe(0);
    expect(activity.failed).toEqual([{ path: `/functions/${"x".repeat(300)}`.slice(0, MAX_FAILED_PATH), status: 500 }]);
  });
});

describe("observe", () => {
  it("posts nothing outside a frame", async () => {
    await expect(observe({ ...FAST, win: window })).resolves.toBeNull();
    const post = vi.spyOn(window, "postMessage");
    startObserver({ ...FAST, win: window });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(post).not.toHaveBeenCalled();
  });

  it("walks the navigation page by page and returns to the page the person was on", async () => {
    document.body.innerHTML = `
      <nav>
        <button type="button" id="overview" aria-current="page">Overview</button>
        <button type="button" id="stations">Stations</button>
        <a href="https://example.test/elsewhere">Leaves</a>
      </nav>
      <main id="main"><h1>Overview</h1><p>5 stations</p></main>`;
    const main = document.getElementById("main")!;
    const overview = document.getElementById("overview")!;
    const stations = document.getElementById("stations")!;
    const clicks: string[] = [];
    overview.addEventListener("click", () => {
      clicks.push("overview");
      main.innerHTML = "<h1>Overview</h1><p>5 stations</p>";
    });
    stations.addEventListener("click", () => {
      clicks.push("stations");
      main.innerHTML = "<table><tbody><tr><td>Kaivopuisto</td></tr><tr><td>Viiskulma</td></tr></tbody></table>";
    });
    activity.failed.push({ path: "/functions/summary", status: 500 });

    const { win } = frameWindow("?v=3");
    const observation = await observe({ ...FAST, win });

    expect(clicks).toEqual(["overview", "stations", "overview"]);
    expect(observation?.version).toBe(3);
    expect(observation?.pages.map((page) => page.label)).toEqual(["Overview", "Stations"]);
    expect(observation?.pages[0].text).toContain("5 stations");
    expect(observation?.pages[1].text).toContain("Kaivopuisto");
    expect(observation?.pages[1].rows).toEqual([2]);
    expect(observation?.failedRequests).toEqual([{ path: "/functions/summary", status: 500 }]);
  });

  it("walks nothing until the host page asks, and says it is listening", async () => {
    document.body.innerHTML = `<nav><button id="stations">Stations</button></nav><main>Overview</main>`;
    const clicks: string[] = [];
    document.getElementById("stations")!.addEventListener("click", () => clicks.push("stations"));
    const { win, parent, send } = frameWindow("?v=4");
    startObserver({ ...FAST, win });
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(clicks).toEqual([]);
    expect(parent.postMessage.mock.calls).toEqual([[{ kind: "jc-ready" }, "*"]]);

    // Another window cannot start the walk, only the page that framed the preview.
    send({ kind: "jc-observe", version: 4 }, { postMessage: vi.fn() });
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(clicks).toEqual([]);

    send({ kind: "jc-observe", version: 4 });
    await vi.waitFor(() => expect(parent.postMessage).toHaveBeenCalledTimes(2));
    expect(clicks).toEqual(["stations"]);
    expect(parent.postMessage.mock.calls[1][0]).toMatchObject({ kind: "jc-observation", version: 4 });
  });

  it("reads one page when there is no navigation, and posts it once to the host page", async () => {
    document.title = "Bikes";
    document.body.innerHTML = "<p>NaN bikes</p>";
    const { win, parent, send } = frameWindow("?x=1&v=12");
    startObserver({ ...FAST, win });
    startObserver({ ...FAST, win });
    send({ kind: "jc-observe", version: 12 });
    send({ kind: "jc-observe", version: 12 });
    await vi.waitFor(() => expect(parent.postMessage).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(parent.postMessage).toHaveBeenCalledTimes(2);
    const [message, origin] = parent.postMessage.mock.calls[1];
    expect(origin).toBe("*");
    expect(message).toEqual({
      kind: "jc-observation",
      version: 12,
      pages: [{ label: "Bikes", text: "NaN bikes", rows: [], h1: 0 }],
      failedRequests: [],
    });
  });

  it("keeps every value inside the route's bounds", () => {
    document.body.innerHTML = `<nav>${Array.from({ length: MAX_PAGES + 5 }, (_, i) => `<button>${i}${"P".repeat(MAX_LABEL + 10)}</button>`).join("")}</nav>
      <p>${"a".repeat(MAX_TEXT + 500)}</p>
      ${"<table><tbody><tr><td>1</td></tr></tbody></table>".repeat(60)}`;
    const items = navItems(document);
    expect(items).toHaveLength(MAX_PAGES);
    expect(items.every((item) => item.label.length <= MAX_LABEL)).toBe(true);
    const page = snapshot(document, "x".repeat(500));
    expect(page.label).toHaveLength(MAX_LABEL);
    expect(page.text.length).toBeLessThanOrEqual(MAX_TEXT);
    expect(page.rows).toHaveLength(50);
  });

  it("counts the level-1 headings of a page and nothing else of them (T-3060)", () => {
    document.body.innerHTML = "<main><h2>Stations</h2></main>";
    expect(snapshot(document, "A").h1).toBe(0);
    document.body.innerHTML = '<header><h1>City bikes</h1></header><main><h2>Stations</h2></main>';
    expect(snapshot(document, "A").h1).toBe(1);
    document.body.innerHTML = '<h1>City bikes</h1><div role="heading" aria-level="1">Stations</div>';
    expect(snapshot(document, "A").h1).toBe(2);
    document.body.innerHTML = "<h1>x</h1>".repeat(MAX_H1 + 5);
    expect(snapshot(document, "A").h1).toBe(MAX_H1);
  });

  it("reads the version from the preview URL only when it is a positive number", () => {
    expect(versionOf("?v=2")).toBe(2);
    expect(versionOf("?a=1&v=40")).toBe(40);
    expect(versionOf("?v=0")).toBeUndefined();
    expect(versionOf("?v=2x")).toBeUndefined();
    expect(versionOf("")).toBeUndefined();
  });
});
