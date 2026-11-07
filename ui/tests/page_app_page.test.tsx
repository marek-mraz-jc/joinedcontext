/**
 * T-1779: an application's page against the page contract (UI-11, UI-15, UI-16, AP-68, AP-69).
 *
 * `AppPage` is the four states of one question — does this application have a run yet — and the
 * two whole pages it hands over to. `app_route.test.tsx` keeps what it hands over to; what is
 * asserted here is that each state is a page: named in the heading and in the tab, with the way
 * back, a reason that can be acted on when the request fails, and axe clean in each.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AppPage } from "../src/pages/apps/AppPage";
import { pageFindings } from "./pageChecks";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  list,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";
const APP = "ovzdusie-dnes";
/** The name read as words, which is what the page says before a run names the application. */
const AS_WORDS = "Ovzdusie dnes";

/** One blueprint, which is what the generator needs before it can offer its form (AP-68). */
const BLUEPRINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Blueprint",
  metadata: { name: "app-from-prompt", namespace: "org", title: { en: "App from a prompt" } },
  spec: { version: "1.4.0", riskClass: "yellow" },
};

/** A published App the platform shipped, with no build run of its own (T-4160 #35). */
const PUBLISHED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "App",
  metadata: {
    name: APP,
    namespace: PROJECT,
    title: { en: "Air today", sk: "Ovzdušie dnes" },
    description: { en: "Today's PM10 at every station." },
  },
  spec: { lifecycle: "published", visibility: "public" },
};

/** The same App built without a description, as a generated App can be (T-6527). */
const UNDESCRIBED = { ...PUBLISHED, metadata: { name: APP, namespace: PROJECT, title: { en: "Air today" } } };

interface World {
  app?: unknown;
  runs?: number;
  fails?: { status: number; detail: string };
  pending?: boolean;
}

function renderApp(world: World = {}) {
  const { app, runs = 0, fails, pending = false } = world;
  return renderPage(<AppPage project={PROJECT} name={APP} />, {
    path: `/projects/${PROJECT}/apps/${APP}`,
    answer: (url, request) => {
      if (url.pathname.endsWith("/blueprints")) return json(list([BLUEPRINT]));
      if (app && url.pathname.endsWith(`/apps/${APP}`)) return json(app);
      if (!url.pathname.endsWith("/agent-runs") || request.method !== "GET") return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json({
        items: Array.from({ length: runs }, (_, index) => ({
          id: `01J8ZQ4T7K9M2N3P4Q5R6S7T8${index}`,
          project: PROJECT,
          appName: APP,
          appClass: "fullstack",
          visibility: "project",
          prompt: "A map of the stations with today's PM10",
          status: "building",
          createdAt: "2026-09-12T08:00:00Z",
        })),
      });
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.stubGlobal("EventSource", undefined);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("an application's page", () => {
  // UI-01: the walker's purpose line holds for an App whose manifest says nothing about itself.
  it("keeps a purpose line for an App with no description", async () => {
    renderApp({ app: UNDESCRIBED });
    const heading = await screen.findByRole("heading", { level: 1, name: "Air today" });
    expect(heading.nextElementSibling).toHaveTextContent(en.apps.pagePurpose);
    expect(pageFindings({ namespaces: Object.keys(en), layout: false }).filter((finding) => finding.startsWith("purpose-line"))).toEqual([]);
  });

  // UI-15: the application is named before its runs are known, on screen and in the tab.
  it("names the application while its runs are still loading", async () => {
    renderApp({ pending: true });
    expect(await screen.findByRole("heading", { level: 1, name: AS_WORDS })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
    await waitFor(() => {
      expect(document.title).toBe(`${AS_WORDS} · ${PROJECT} · Helsinki Region Context`);
    });
  });

  // The waiting state draws the shape of what is coming rather than a line of text, and the
  // grey bars are decorative: a screen reader hears "Loading", not three empty regions.
  it("draws the shape of what is coming without reading it out", async () => {
    const { container } = renderApp({ pending: true });
    await screen.findByRole("heading", { level: 1, name: AS_WORDS });
    const bars = container.querySelectorAll("[aria-hidden='true']");
    expect(bars.length).toBeGreaterThan(0);
    await expectNoAxeViolations(container);
  });

  // A failure is a page, not a red line: the API's own sentence, a retry, and the way back.
  it("shows the reason the runs could not be read, with a retry and the way back", async () => {
    renderApp({ fails: { status: 503, detail: "The run store is not answering." } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The run store is not answering.");
    expect(screen.getByRole("heading", { level: 1, name: AS_WORDS })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.apps.back })).toBeInTheDocument();
  });

  // UI-44: refused says who refused it and the page still stands.
  it("keeps the page when the runs are refused", async () => {
    renderApp({ fails: { status: 403, detail: "You may not read this project's applications." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this project's applications.",
    );
    expect(screen.getByRole("heading", { level: 1, name: AS_WORDS })).toBeInTheDocument();
  });

  // The retry asks again rather than reloading the page, and a second answer replaces the error.
  it("asks again when the retry is pressed", async () => {
    let failing = true;
    renderPage(<AppPage project={PROJECT} name={APP} />, {
      path: `/projects/${PROJECT}/apps/${APP}`,
      answer: (url, request) => {
        if (!url.pathname.endsWith("/agent-runs") || request.method !== "GET") return undefined;
        if (failing) return problem(503, "The run store is not answering.");
        return json({ items: [] });
      },
    });
    const user = userEvent.setup();
    await screen.findByRole("alert");
    failing = false;
    await user.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("heading", { name: en.apps.generate.title })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  // The empty state of an application is the form that creates its first run, prefilled.
  it("offers the first run when the application has none", async () => {
    const { container } = renderApp({ runs: 0 });
    expect(
      await screen.findByRole("heading", { name: en.apps.generate.title }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(en.apps.generate.name)).toHaveValue(APP);
    expectHeadingOutline(container);
  });

  // UI-16: the way back is the first thing the keyboard reaches, and nothing takes focus on
  // arrival.
  it("reaches the way back by keyboard first", async () => {
    const { container } = renderApp({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    expect(reached[0]).toBe(screen.getByRole("button", { name: en.apps.back }));
    expect(reached).toContain(screen.getByRole("button", { name: en.app.error.retry }));
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderApp({ fails: { status: 503, detail: "Not answering." } });
      expect(
        await screen.findByRole("button", { name: i18n.t("apps.back") }),
        `the way back is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(
        await screen.findByRole("button", { name: i18n.t("app.error.retry") }),
      ).toBeInTheDocument();
    });
  });

  // PF-50: a detail the API wrote is read as text, never parsed as markup.
  it("renders a refusal that arrived as markup as text", async () => {
    renderApp({ fails: { status: 403, detail: "<img src=x onerror=alert(1)>" } });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderApp({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });

  it("has no axe violations with the first-run form", async () => {
    const { container } = renderApp({ runs: 0 });
    await screen.findByRole("heading", { name: en.apps.generate.title });
    await expectNoAxeViolations(container);
  });
});

describe("a published App with no run of its own (T-4264, T-4160 #35)", () => {
  it("opens on the App's title, state and audience, with the generator behind its action", async () => {
    renderApp({ app: PUBLISHED });
    expect(await screen.findByRole("heading", { level: 1, name: "Air today" })).toBeInTheDocument();
    expect(screen.getByText("Today's PM10 at every station.")).toBeInTheDocument();
    expect(screen.getByText(en.apps.visibility.replace("{visibility}", en.apps.audience.public))).toBeInTheDocument();
    const action = screen.getByText(en.apps.editWithAssistant);
    expect(action.closest("details")).not.toHaveAttribute("open");
    // One H1: the generator under it is a section of the App's page.
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 2, name: en.apps.generate.title, hidden: true })).toBeInTheDocument();
  });
});

