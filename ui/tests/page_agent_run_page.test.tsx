/**
 * T-1778: the run page against the page contract (UI-11, UI-15, UI-16, UI-42, AP-19, AP-63).
 *
 * `agent_run.test.tsx` keeps what a person cannot check by looking: the replay, the publishing
 * rule, the frame's sandbox. What is asserted here is the frame around them — that the page is a
 * page in each of its four states, that its heading outline, its keyboard order and its four
 * languages hold, that axe is clean while the preview stands and while it is still building, and
 * that the preview column is the same box in both so the page does not jump under a reader.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AgentRunPage } from "../src/pages/apps/AgentRunPage";
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
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
/** The name read as words, which is what the page calls the application (AP-69). */
const APP_TITLE = "Ovzdusie dnes";

const RUN = {
  id: RUN_ID,
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 12,
  tokensUsed: 48_210,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

interface World {
  run?: Record<string, unknown>;
  fails?: { status: number; detail: string };
  pending?: boolean;
}

function renderRun(world: World = {}) {
  const { run = RUN, fails, pending = false } = world;
  const closed = vi.fn();
  const view = renderPage(<AgentRunPage project={PROJECT} runId={RUN_ID} onClose={closed} />, {
    path: `/projects/${PROJECT}/apps/${RUN.appName}`,
    answer: (url) => {
      if (url.pathname.endsWith("/endpoints")) return json(list([]));
      if (!url.pathname.includes("/agent-runs/")) return undefined;
      if (pending) return new Promise<Response>(() => undefined);
      if (fails) return problem(fails.status, fails.detail);
      return json(run);
    },
  });
  return { ...view, closed };
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

describe("the run page", () => {
  // UI-15: the page is a page while the record is still on its way — a heading, the way back,
  // and the shape of what is coming instead of one line of text on blank white.
  it("is a page while the run is being read", async () => {
    const { container } = renderRun({ pending: true });
    expect(
      await screen.findByRole("heading", { level: 1, name: en.agentRun.title }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.agentRun.loading);
    expect(screen.getByRole("button", { name: en.agentRun.back })).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  // The reason the record could not be read, and a retry: a refusal and an outage used to read
  // "That run is not in this project" and offer nothing.
  it("shows the reason the run could not be read, with a retry", async () => {
    renderRun({ fails: { status: 503, detail: "The run store is not answering." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The run store is not answering.",
    );
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.agentRun.notFound)).not.toBeInTheDocument();
  });

  // UI-44: refused says who refused it, in the API's words.
  it("says a refusal in the API's own words", async () => {
    renderRun({ fails: { status: 403, detail: "You may not read this project's runs." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this project's runs.",
    );
  });

  // A run that really is gone is the one case that keeps the old sentence, and it offers no
  // retry: asking again would answer 404 again.
  it("says a run that is gone is gone, and offers no retry", async () => {
    renderRun({ fails: { status: 404, detail: "No such run." } });
    expect(await screen.findByRole("alert")).toHaveTextContent(en.agentRun.notFound);
    expect(screen.queryByRole("button", { name: en.app.error.retry })).not.toBeInTheDocument();
  });

  it("reads as one outline with the run in the heading", async () => {
    const { container } = renderRun();
    await screen.findByRole("heading", { level: 1, name: APP_TITLE });
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(`${APP_TITLE} · ${PROJECT} · Helsinki Region Context`);
    });
  });

  // UI-42: the building state and the preview stand in the same box, so the column does not
  // jump under a reader when the first pass arrives.
  it("builds in the same box the preview will take", async () => {
    const { container } = renderRun();
    const building = await screen.findByTestId("run-building");
    expect(building.className).toContain("h-preview");
    expect(building.className).toContain("min-h-preview-min");
    cleanup();

    renderPage(
      <AgentRunPage project={PROJECT} runId={RUN_ID} onClose={() => undefined} />,
      {
        path: `/projects/${PROJECT}/apps/${RUN.appName}`,
        answer: (url) =>
          url.pathname.includes("/agent-runs/")
            ? json({ ...RUN, status: "previewing", previewUrl: `/apps/${PROJECT}/preview/1` })
            : undefined,
      },
    );
    const frame = await waitFor(() => {
      const found = document.querySelector("iframe");
      expect(found).not.toBeNull();
      return found as HTMLIFrameElement;
    });
    expect(frame.className).toContain("h-preview");
    expect(frame.className).toContain("min-h-preview-min");
    expect(container).toBeTruthy();
  });

  // AP-19: the frame never carries the reviewer's session, and it cannot be talked out of it.
  it("frames the preview without same-origin and without a referrer", async () => {
    renderPage(
      <AgentRunPage project={PROJECT} runId={RUN_ID} onClose={() => undefined} />,
      {
        path: `/projects/${PROJECT}/apps/${RUN.appName}`,
        answer: (url) =>
          url.pathname.includes("/agent-runs/")
            ? json({ ...RUN, status: "previewing", previewUrl: `/apps/${PROJECT}/preview/1` })
            : undefined,
      },
    );
    const frame = await waitFor(() => {
      const found = document.querySelector("iframe");
      expect(found).not.toBeNull();
      return found as HTMLIFrameElement;
    });
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-forms");
    expect(frame.getAttribute("referrerPolicy")).toBe("no-referrer");
    expect(frame.getAttribute("title")).toBeTruthy();
  });

  // UI-45: an address the run reported that is not a path under /apps/ on this origin is named,
  // never framed and never made a link.
  it("refuses to frame an address from somewhere else and says what arrived", async () => {
    renderPage(
      <AgentRunPage project={PROJECT} runId={RUN_ID} onClose={() => undefined} />,
      {
        path: `/projects/${PROJECT}/apps/${RUN.appName}`,
        answer: (url) =>
          url.pathname.includes("/agent-runs/")
            ? json({ ...RUN, status: "previewing", previewUrl: "javascript:alert(1)" })
            : undefined,
      },
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("javascript:alert(1)");
    expect(document.querySelector("iframe")).toBeNull();
    expect(document.querySelector('a[href^="javascript:"]')).toBeNull();
  });

  // UI-16: the way back is reached first, and nothing takes focus on arrival.
  it("reaches the way back by keyboard first", async () => {
    const { container } = renderRun({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    expect(reached[0]).toBe(screen.getByRole("button", { name: en.agentRun.back }));
  });

  // T-6442: a failed run says what happened in a person's words, what to do next, and Publish
  // refuses with its reason.
  it("says a failed run's next step and holds Publish back", async () => {
    renderRun({
      run: {
        ...RUN,
        status: "failed",
        error: "The Portal's database did not answer in time. Start the run again in a minute.",
      },
    });
    const alert = await screen.findByText(/did not answer in time/);
    expect(alert.textContent).toContain(en.agentRun.failedNext);
    const publish = screen.getByRole("button", { name: new RegExp(`^${en.agentRun.publish}`) });
    expect(publish).toBeDisabled();
    expect(publish).toHaveAccessibleDescription(en.agentRun.publishOver);
    expect(screen.getByRole("link", { name: en.agentRun.failedOpen })).toHaveAttribute("href", expect.stringMatching(/\/assistant$/));
    expect(screen.queryByRole("button", { name: en.agentRun.cancel })).toBeNull();
    expect(screen.queryByText(/Open its steps/)).toBeNull();
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderRun({ pending: true });
      expect(
        await screen.findByRole("heading", { level: 1, name: i18n.t("agentRun.title") }),
        `the run page has no title in ${locale}`,
      ).toBeInTheDocument();
      expect(screen.getByRole("button", { name: i18n.t("agentRun.back") })).toBeInTheDocument();
    });
  });

  // PF-50: a prompt or a reason that arrived as markup is read as text.
  it("renders a refusal that arrived as markup as text", async () => {
    renderRun({ fails: { status: 500, detail: "<img src=x onerror=alert(1)>" } });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations while the run is building", async () => {
    const { container } = renderRun();
    await screen.findByTestId("run-building");
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderRun({ fails: { status: 503, detail: "Not answering." } });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});
