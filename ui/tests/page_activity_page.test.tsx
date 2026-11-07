/**
 * T-1786: Project → Activity against the page contract (UI-11, UI-15, UI-16, UI-31).
 *
 * `ActivityPage` is the header, the hour's summary and the feed. What it owns is the frame: the
 * H1 and the tab, the heading outline over the two panels, the keyboard reaching the filters and
 * the live tail in reading order, the four languages, and axe over the whole page in each of the
 * states the feed can be in. `activity_feed.test.tsx` keeps the feed's own merging and tailing.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ActivityPage } from "../src/routes/ActivityPage";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";

function event(index: number, severity: "info" | "warning" | "error" = "info") {
  return {
    time: new Date(Date.UTC(2026, 8, 20, 9, 0, index % 60)).toISOString(),
    kind: "reconcile",
    severity,
    source: "reconciler",
    summary: `Applied change ${index}`,
    object: { kind: "ContextSpace", name: `space-${index}` },
  };
}

interface World {
  rows?: number;
  /** A fresh answer per request: a `Response` body can be read once, and two panels ask. */
  fails?: { status: number; detail: string };
  pending?: boolean;
}

function renderActivity(world: World = {}) {
  const { rows = 3, fails, pending = false } = world;
  return renderPage(<ActivityPage project={PROJECT} />, {
    path: `/projects/${PROJECT}/activity`,
    answer: (url) => {
      if (!url.pathname.endsWith("/activity")) return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ActivityList",
        items: Array.from({ length: rows }, (_, index) => event(index)),
      });
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  // No EventSource in jsdom: the feed's tail then subscribes to nothing, which is the state
  // this page is in before the first event arrives anyway.
  vi.stubGlobal("EventSource", undefined);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("the Activity page", () => {
  // T-5608: a link to `?kind=mcp.tool` opens the feed filtered to that kind; a kind this Portal
  // does not know leaves the filter on Any rather than asking for a list nobody can read.
  it.each([
    ["mcp.tool", "mcp.tool"],
    ["no.such.kind", ""],
  ])("opens with the kind of its address, %s", async (kind, shown) => {
    const asked: URL[] = [];
    renderPage(<ActivityPage project={PROJECT} kind={kind} />, {
      path: `/projects/${PROJECT}/activity?kind=${kind}`,
      answer: (url) => {
        if (!url.pathname.endsWith("/activity")) return undefined;
        asked.push(url);
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ActivityList", items: [] });
      },
    });
    expect(await screen.findByLabelText(en.activity.filter.kind)).toHaveValue(shown);
    await waitFor(() => expect(asked.length).toBeGreaterThan(0));
    // The hour's summary asks for every kind; the feed asks for the one shown, never the unknown.
    expect(asked.some((url) => (url.searchParams.get("kind") ?? "") === shown)).toBe(true);
    expect(asked.some((url) => url.searchParams.get("kind") === "no.such.kind")).toBe(false);
  });

  // UI-15: the page names itself once, to the screen and to the tab.
  it("has one H1 and names the page and the project in the tab", async () => {
    renderActivity();
    expect(
      await screen.findByRole("heading", { level: 1, name: en.activity.title }),
    ).toBeInTheDocument();
    expect(screen.getByText(en.activity.lead)).toBeInTheDocument();
    await waitFor(() => {
      expect(document.title).toBe(`${en.activity.title} · ${PROJECT} · Helsinki Region Context`);
    });
  });

  it("reads as one outline over the summary and the feed", async () => {
    const { container } = renderActivity();
    await screen.findByRole("heading", { level: 1, name: en.activity.title });
    await screen.findByRole("heading", { level: 2, name: en.activity.lastHour });
    expectHeadingOutline(container);
  });

  it("says the feed is loading rather than showing it empty", async () => {
    renderActivity({ pending: true });
    expect(await screen.findByText(en.app.loading)).toBeInTheDocument();
    expect(screen.queryByText(en.activity.empty)).not.toBeInTheDocument();
  });

  it("says what lands here when nothing has happened yet", async () => {
    renderActivity({ rows: 0 });
    expect(await screen.findByText(en.activity.empty)).toBeInTheDocument();
    expect(screen.getByText(en.activity.emptyHint)).toBeInTheDocument();
  });

  // A list that failed is not "you have nothing": the API's own sentence, and a retry.
  it("shows the reason the feed failed, with a retry", async () => {
    renderActivity({ fails: { status: 502, detail: "The activity store is not answering." } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The activity store is not answering.");
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.activity.empty)).not.toBeInTheDocument();
  });

  // UI-44: refused is said in words, and the page still stands.
  it("keeps its header when the feed is refused", async () => {
    renderActivity({ fails: { status: 403, detail: "You may not read this project's activity." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this project's activity.",
    );
    expect(screen.getByRole("heading", { level: 1, name: en.activity.title })).toBeInTheDocument();
  });

  it.each([0, 1, 500])("draws %i events without changing how the page reads", async (rows) => {
    const { container } = renderActivity({ rows });
    await screen.findByRole("heading", { level: 1, name: en.activity.title });
    if (rows === 0) {
      expect(await screen.findByText(en.activity.empty)).toBeInTheDocument();
    } else {
      // The feed asks for 50 at a time and draws what it holds: what matters is that one event
      // and five hundred both read as the same page, and that 500 rows are never put in front
      // of a person at once.
      expect(await screen.findByText("Applied change 0")).toBeInTheDocument();
      const table = await screen.findByRole("table", { name: en.activity.title });
      const drawn = table.querySelectorAll("tbody tr").length;
      expect(drawn).toBe(Math.min(rows, 50));
    }
    expectHeadingOutline(container);
  });

  // UI-16: the filters and the live tail are reached by keyboard, in reading order, and nothing
  // takes focus on arrival.
  it("reaches the filters and the live tail by keyboard", async () => {
    const { container } = renderActivity();
    await screen.findByLabelText(en.activity.filter.kind);
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    expect(reached).toContain(screen.getByLabelText(en.activity.filter.kind));
    expect(reached).toContain(screen.getByLabelText(en.activity.filter.source));
    expect(reached).toContain(screen.getByLabelText(en.activity.filter.severity));
    expect(reached.indexOf(screen.getByLabelText(en.activity.filter.kind))).toBeLessThan(
      reached.indexOf(screen.getByLabelText(en.activity.filter.severity)),
    );
  });

  // UI-31: the live tail is a switch a keyboard can turn off, and turning it off says so.
  it("turns the live tail off from the keyboard", async () => {
    renderActivity();
    const user = userEvent.setup();
    const tail = await screen.findByRole("switch", { name: en.activity.tail });
    expect(screen.getByText(en.activity.tailOn)).toBeInTheDocument();
    tail.focus();
    await user.keyboard("{ }");
    expect(await screen.findByText(en.activity.tailOff)).toBeInTheDocument();
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderActivity({ rows: 0 });
      const title = i18n.t("activity.title");
      expect(await screen.findByRole("heading", { level: 1, name: title })).toBeInTheDocument();
      expect(
        await screen.findByText(i18n.t("activity.emptyHint")),
        `the empty hint is missing in ${locale}`,
      ).toBeInTheDocument();
    });
  });

  // PF-50: a summary the platform wrote is read as text, and the object it names is a path this
  // page builds, never a URL an event carried.
  it("renders an event summary that arrived as markup as text", async () => {
    renderPage(<ActivityPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/activity`,
      answer: (url) =>
        url.pathname.endsWith("/activity")
          ? json({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ActivityList",
              items: [{ ...event(1), summary: "<img src=x onerror=alert(1)>" }],
            })
          : undefined,
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with events in the feed", async () => {
    const { container } = renderActivity({ rows: 5 });
    await screen.findByRole("table", { name: en.activity.title });
    await expectNoAxeViolations(container);
  });

  it("has no axe violations while it is empty", async () => {
    const { container } = renderActivity({ rows: 0 });
    await screen.findByText(en.activity.empty);
    await expectNoAxeViolations(container);
  });
});
