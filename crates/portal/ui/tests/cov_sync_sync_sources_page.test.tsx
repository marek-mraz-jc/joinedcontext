// T-7589: coverage for SyncSourcesPage
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SyncSourcesPage } from "../src/pages/sync/SyncSourcesPage";
import { json, list, problem, renderPage } from "./page_contract";

const PROJECT = "helsinki";

const PERMISSIONS = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      role: "admin",
      binding: "admins",
      rule: { kinds: ["SyncSource"], verbs: ["read", "propose", "delete"] },
    },
  ],
};

const DEFAULT_STATUS = {
  phase: "Live",
  observedRevision: "9c1f0ab2233445566778899aabbccddeeff00112",
  durable: true,
  paused: false,
};

interface RenderOptions {
  items?: Record<string, unknown>[];
  status?: Record<string, unknown> | "fails";
  fails?: { status: number; detail: string };
  onAnswer?: (url: URL, request: Request) => Response | undefined | Promise<Response | undefined>;
}

function renderSync(options: RenderOptions = {}) {
  const { items = [], status = DEFAULT_STATUS, fails, onAnswer } = options;
  return renderPage(<SyncSourcesPage project={PROJECT} />, {
    path: `/projects/${PROJECT}/syncsources`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json(PERMISSIONS);
      }
      if (url.pathname.endsWith("/projects/helsinki") || url.pathname.endsWith("projects('helsinki')")) {
        return new Response(JSON.stringify({ metadata: { name: "helsinki" } }), {
          status: 200,
          headers: { "Content-Type": "application/json", ETag: '"etag-1"' },
        });
      }
      if (request.method === "PUT" && url.pathname.includes("/drafts/")) {
        return json({ version: 1, manifest: null, verdict: null, touchedBy: "jana.kovacova", touchedKind: "person" });
      }
      if (url.pathname.includes("/drafts/")) {
        return json({ title: "Not Found", status: 404 }, 404);
      }
      if (onAnswer) {
        const answered = await onAnswer(url, request);
        if (answered) return answered;
      }
      if (url.pathname.includes("/status")) {
        return status === "fails"
          ? problem(502, "The source's repository did not answer.")
          : json(status);
      }
      if (url.pathname.endsWith("/syncsources") && request.method === "GET") {
        if (fails) return problem(fails.status, fails.detail);
        return json(list(items));
      }
      return undefined;
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("SyncSourcesPage origin select, EmptyState add button, and dialog cancellation", () => {
  it("changes origin kind select and opens dialog from EmptyState button, then closes on cancel", async () => {
    const user = userEvent.setup();
    renderSync({ items: [] });

    // Verify EmptyState is shown
    expect(await screen.findByText(en.syncSources.empty)).toBeInTheDocument();

    // Select dropdown onChange (line 153-154)
    const originSelect = screen.getByLabelText(en.syncSources.addFrom);
    expect(originSelect).toHaveValue("git");
    fireEvent.change(originSelect, { target: { value: "bundle" } });
    expect(originSelect).toHaveValue("bundle");
    fireEvent.change(originSelect, { target: { value: "platformApi" } });
    expect(originSelect).toHaveValue("platformApi");

    // Click the Add button inside EmptyState (line 223)
    const emptyStateHeading = screen.getByText(en.syncSources.empty);
    const emptyStateContainer = emptyStateHeading.closest("div")!;
    const emptyStateAddBtn = within(emptyStateContainer).getByRole("button", { name: en.syncSources.add });
    await user.click(emptyStateAddBtn);

    // Dialog opens
    const dialog = await screen.findByRole("dialog", { name: en.syncSources.dialog.title });
    expect(dialog).toBeInTheDocument();

    // Cancel the dialog: onOpenChange(false) runs lines 181-186
    const cancelBtn = within(dialog).getByRole("button", { name: en.form.cancel });
    await user.click(cancelBtn);
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });
});

describe("SyncSourcesPage PageFailed retry", () => {
  it("triggers refetch when clicking the retry button in PageFailed", async () => {
    const user = userEvent.setup();
    let fetchCount = 0;
    renderSync({
      fails: { status: 500, detail: "Configuration store unavailable" },
      onAnswer: (url) => {
        if (url.pathname.endsWith("/syncsources")) {
          fetchCount += 1;
        }
        return undefined;
      },
    });

    const retryBtn = await screen.findByRole("button", { name: en.app.error.retry });
    const countBefore = fetchCount;
    // Clicking retry triggers onRetry (line 213)
    await user.click(retryBtn);
    await waitFor(() => expect(fetchCount).toBeGreaterThan(countBefore));
  });
});

describe("SyncSourcesPage detach and review link", () => {
  it("confirms detach, calls detach mutation, handles onSuccess with review url and settles dialog", async () => {
    const user = userEvent.setup();
    let detachExecuted = false;

    renderSync({
      items: [
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "SyncSource",
          metadata: { name: "src-detach", namespace: PROJECT },
          spec: { source: { git: { url: "https://git.example.com/repo" } } },
        },
      ],
      onAnswer: (url, request) => {
        if (request.method === "POST" && (url.pathname.includes("detach") || url.pathname.includes("jc_syncsource_detach"))) {
          detachExecuted = true;
          return json({ url: "https://git.example.com/city/org/pulls/88" });
        }
        return undefined;
      },
    });

    const detachBtn = await screen.findByRole("button", { name: en.syncSources.detach });
    await user.click(detachBtn);

    const dialog = await screen.findByRole("alertdialog", { name: en.syncSources.detach });
    const confirmBtn = within(dialog).getByRole("button", { name: en.syncSources.detach });

    // Confirm detach runs lines 420, 282, 285-287
    await user.click(confirmBtn);

    expect(detachExecuted).toBe(true);
    expect(await screen.findByText(en.syncSources.detached)).toBeInTheDocument();
    const link = screen.getByRole("link", { name: new RegExp(en.syncSources.review) });
    expect(link).toHaveAttribute("href", "https://git.example.com/city/org/pulls/88");
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("handles detach succeeding with no url", async () => {
    const user = userEvent.setup();
    renderSync({
      items: [
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "SyncSource",
          metadata: { name: "src-detach-nourl", namespace: PROJECT },
          spec: { source: { git: { url: "https://git.example.com/repo" } } },
        },
      ],
      onAnswer: (url, request) => {
        if (request.method === "POST" && (url.pathname.includes("detach") || url.pathname.includes("jc_syncsource_detach"))) {
          return json({ url: null });
        }
        return undefined;
      },
    });

    const detachBtn = await screen.findByRole("button", { name: en.syncSources.detach });
    await user.click(detachBtn);
    const dialog = await screen.findByRole("alertdialog", { name: en.syncSources.detach });
    const confirmBtn = within(dialog).getByRole("button", { name: en.syncSources.detach });
    await user.click(confirmBtn);

    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(screen.queryByText(en.syncSources.detached)).toBeNull();
  });
});
