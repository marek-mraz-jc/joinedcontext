/**
 * T-1785: "Try the copy" against the page contract (UI-11, UI-15, UI-16, UI-44, UI-61, PF-83).
 *
 * `try_it.test.tsx` keeps starting and stopping a preview and the bounded copy of data. What is
 * asserted here is that the page is a page in each of its four states — until this task it had
 * none: both of its requests could fail and the page still drew "Not started" with a Start
 * button that was going to be refused as well — and that its addresses, which the copy reports,
 * are links only when they are addresses at all.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { TryItPage } from "../src/pages/workspaces/TryItPage";
import {
  expectHeadingOutline,
  expectNoAxeViolations,
  inEveryLocale,
  json,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

let me: { email: string; username: string } = { email: "jana@hel.fi", username: "jana" };
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated",
  useAuth: () => ({ identity: me, status: "authenticated" }),
}));

const PROJECT = "helsinki";
const COPY = "air-v2";
const WORKSPACE = { name: COPY, title: "Air cleanup", project: PROJECT, owner: "jana@hel.fi" };

const RUNNING = {
  state: "running",
  prefix: "ws-air-v2-",
  endpoints: [
    {
      name: "public-air",
      slug: "minted",
      url: "https://preview.example.sk/api/endpoint/minted",
      originSlug: "origin",
    },
  ],
  pausedPipelines: ["air-feed"],
};

interface World {
  preview?: Record<string, unknown>;
  fails?: { on: "workspace" | "preview"; status: number; detail: string };
  pending?: boolean;
}

function renderTryIt(world: World = {}) {
  const { preview = RUNNING, fails, pending = false } = world;
  return renderPage(<TryItPage project={PROJECT} name={COPY} />, {
    path: `/projects/${PROJECT}/workspaces/${COPY}/try-it`,
    answer: (url, request) => {
      const isPreview = url.pathname.endsWith("/preview");
      const isWorkspace = url.pathname.endsWith(`/workspaces/${COPY}`);
      if (!isPreview && !isWorkspace) return undefined;
      if (isPreview && request.method !== "GET") return json(RUNNING, 202);
      if (pending) return new Promise<Response>(() => undefined);
      if (fails && ((fails.on === "preview") === isPreview)) {
        return problem(fails.status, fails.detail);
      }
      return isPreview ? json(preview) : json(WORKSPACE);
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  me = { email: "jana@hel.fi", username: "jana" };
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("trying a copy", () => {
  it("has one H1 and names the page and the project in the tab", async () => {
    const { container } = renderTryIt();
    expect(
      await screen.findByRole("heading", { level: 1, name: en.workspaces.tryIt.title }),
    ).toBeInTheDocument();
    expectHeadingOutline(container);
    await waitFor(() => {
      expect(document.title).toBe(
        `${en.workspaces.tryIt.title} · ${PROJECT} · Helsinki Region Context`,
      );
    });
  });

  it("keeps the heading while the copy and its preview are read", async () => {
    renderTryIt({ pending: true });
    expect(
      await screen.findByRole("heading", { level: 1, name: en.workspaces.tryIt.title }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
    expect(screen.queryByText(en.workspaces.previewStates.none)).not.toBeInTheDocument();
  });

  // Both requests can fail on their own, and neither failure is "the preview is not running".
  it.each([
    ["workspace", 403, "You may not read this copy."],
    ["preview", 503, "The preview controller is not answering."],
  ] as const)("says why %s could not be read, instead of drawing no preview", async (on, status, detail) => {
    renderTryIt({ fails: { on, status, detail } });
    expect(await screen.findByRole("alert")).toHaveTextContent(detail);
    expect(screen.queryByText(en.workspaces.previewStates.none)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.workspaces.tryIt.start }),
    ).not.toBeInTheDocument();
    // A refusal answers the same the second time (T-2834); the way back is there either way.
    expect(screen.queryAllByRole("button", { name: en.app.error.retry })).toHaveLength(status >= 500 ? 1 : 0);
    expect(screen.getByRole("link", { name: en.workspaces.compare.back })).toBeInTheDocument();
  });

  it("says where a running preview answers and what stays paused", async () => {
    const { container } = renderTryIt();
    expect(await screen.findByTestId("preview-state")).toHaveTextContent(
      en.workspaces.previewStates.running,
    );
    expect(
      screen.getByRole("link", { name: /preview\.example\.sk/ }),
    ).toHaveAttribute("href", RUNNING.endpoints[0].url);
    expect(screen.getByText("air-feed")).toBeInTheDocument();
    expectHeadingOutline(container);
  });

  it("says a copy with nothing published has no addresses and no pipelines", async () => {
    const { container } = renderTryIt({
      preview: { state: "running", prefix: "ws-", endpoints: [], pausedPipelines: [] },
    });
    expect(await screen.findByText(en.workspaces.tryIt.noEndpoints)).toBeInTheDocument();
    expect(screen.getByText(en.workspaces.tryIt.noPipelines)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  // PF-50: an address the copy reported that is not one is shown as words, never as a link.
  it("shows an address that is not http as words rather than a link", async () => {
    renderTryIt({
      preview: {
        state: "running",
        prefix: "ws-",
        endpoints: [{ name: "public-air", slug: "minted", url: "javascript:alert(1)" }],
        pausedPipelines: [],
      },
    });
    expect(await screen.findByText("javascript:alert(1)")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /javascript/ })).not.toBeInTheDocument();
    expect(document.querySelector('a[href^="javascript:"]')).toBeNull();
  });

  // UI-44: someone else's copy is read only, with the reason, and nothing to press.
  it("offers someone else's copy no start and says why", async () => {
    me = { email: "erik@hel.fi", username: "erik" };
    renderTryIt({ preview: { state: "stopped", prefix: "ws-", endpoints: [], pausedPipelines: [] } });
    expect(await screen.findByText(en.workspaces.tryIt.notOwner)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.workspaces.tryIt.start }),
    ).not.toBeInTheDocument();
  });

  // The loader's own reason for a preview that failed, which is not the Portal's.
  it("gives the loader's reason when the preview failed", async () => {
    renderTryIt({
      preview: {
        state: "error",
        prefix: "ws-",
        endpoints: [],
        pausedPipelines: [],
        reason: "The broker refused the copy's namespace.",
      },
    });
    expect(await screen.findByText("The broker refused the copy's namespace.")).toBeInTheDocument();
    expect(screen.getByTestId("preview-state")).toHaveTextContent(
      en.workspaces.previewStates.error,
    );
  });

  it("is reachable by keyboard and takes no focus on arrival", async () => {
    const { container } = renderTryIt({
      preview: { state: "stopped", prefix: "ws-", endpoints: [], pausedPipelines: [] },
    });
    await screen.findByRole("button", { name: en.workspaces.tryIt.start });
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    expect(reached[0]).toBe(screen.getByRole("button", { name: en.workspaces.tryIt.start }));
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderTryIt();
      expect(
        await screen.findByRole("heading", { level: 1, name: i18n.t("workspaces.tryIt.title") }),
        `the title is missing in ${locale}`,
      ).toBeInTheDocument();
      expect(
        await screen.findByRole("heading", { level: 2, name: i18n.t("workspaces.tryIt.addresses") }),
      ).toBeInTheDocument();
    });
  });

  it("has no axe violations with a running preview", async () => {
    const { container } = renderTryIt();
    await screen.findByTestId("preview-state");
    await expectNoAxeViolations(container);
  });

  it("has no axe violations in the failed state", async () => {
    const { container } = renderTryIt({
      fails: { on: "preview", status: 503, detail: "Not answering." },
    });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});
