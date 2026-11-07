import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { digestOf } from "../src/api/digest";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { findFormPage } from "./formPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-approver"],
};

const SOURCES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "SyncSource",
      metadata: { name: "regional-datamodels", namespace: "banskabystrica" },
      spec: {
        source: {
          git: {
            url: "https://git.region.sk/udp/datamodels.git",
            ref: "main",
            path: "models/transport",
          },
        },
        schedule: { interval: "30m" },
        mode: "mirror",
      },
    },
  ],
};

const PENDING = {
  project: "banskabystrica",
  name: "regional-datamodels",
  phase: "PendingApproval",
  observedRevision: null,
  lastRunAt: 1757000000,
  mergeRequest: "https://forge.example/pulls/7",
  lastError: null,
  paused: false,
  durable: true,
};

/** Renders the sync view with a status the test chooses, and records what was called. */
function renderSyncSources(status: Record<string, unknown> = PENDING) {
  const calls: { method: string; path: string; body?: string }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const body = request.method === "GET" ? undefined : await request.clone().text();
    calls.push({ method: request.method, path, body });
    const json = (payload: unknown, code = 200) =>
      new Response(JSON.stringify(payload), {
        status: code,
        headers: { "Content-Type": "application/json" },
      });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json({
        instanceName: "joinedcontext",
        languages: { default: "en", offered: ["en"] },
      });
    }
    if (path.endsWith("/status")) {
      return json(status);
    }
    if (path.endsWith("/pause")) {
      return json({ ...status, paused: !status.paused });
    }
    if (path.endsWith("/detach")) {
      return json({ mergeRequest: "https://forge.example/pulls/9", number: 9 }, 202);
    }
    if (path.endsWith("/syncsources/regional-datamodels/sync")) {
      return json({
        proposed: [],
        unchanged: 1,
        flags: [],
        status,
      });
    }
    if (path.endsWith("/syncsources")) {
      return json(SOURCES);
    }
    // The form's Check saves its shared draft, and the Portal answers with a verdict fresh for the
    // manifest it holds; under strict validation nothing is proposed without it (PF-57, T-2731).
    const draft = /\/drafts\/SyncSource\/([^/]+)$/.exec(path);
    if (request.method === "PUT" && draft) {
      const { manifest } = JSON.parse(body ?? "{}") as { manifest: unknown };
      return json({
        kind: "SyncSource",
        name: draft[1],
        project: "helsinki",
        manifest,
        touchedBy: IDENTITY.username,
        touchedKind: "person",
        updatedAt: new Date().toISOString(),
        verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(manifest) },
        version: 1,
      });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return calls;
}

describe("sync sources view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/syncsources");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says where a source reads from, how often, and where it stands (MF-27, MF-30)", async () => {
    renderSyncSources();

    const card = (await screen.findByText("regional-datamodels")).closest(
      "article",
    ) as HTMLElement;
    expect(
      within(card).getByText("https://git.region.sk/udp/datamodels.git#main (models/transport)"),
    ).toBeInTheDocument();
    expect(within(card).getByText("Every 30 minutes")).toBeInTheDocument();
    expect(await within(card).findByText(en.phase.pendingApproval)).toBeInTheDocument();
    expect(await within(card).// `ExternalLink` appends "Opens in a new tab" for a screen reader, so the name is a prefix.
      findByRole("link", { name: new RegExp(`^${en.syncSources.review}`) })).toHaveAttribute(
      "href",
      "https://forge.example/pulls/7",
    );
  });

  it("says why a run failed instead of leaving it to the log (MF-30)", async () => {
    renderSyncSources({
      ...PENDING,
      phase: "Error",
      mergeRequest: null,
      lastError: "sync origin unavailable: the origin did not answer in time",
    });

    const card = (await screen.findByText("regional-datamodels")).closest(
      "article",
    ) as HTMLElement;
    expect(await within(card).findByText(en.phase.error)).toBeInTheDocument();
    expect(
      await within(card).findByText("sync origin unavailable: the origin did not answer in time"),
    ).toBeInTheDocument();
  });

  it("runs one source when Sync now is pressed, and says a quiet run changed nothing", async () => {
    const calls = renderSyncSources();
    await screen.findByText("regional-datamodels");

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.syncNow }));

    await waitFor(() => {
      expect(
        calls.some(
          (call) =>
            call.method === "POST" &&
            call.path ===
              "/api/v1/projects/banskabystrica/syncsources/regional-datamodels/sync",
        ),
      ).toBe(true);
    });
    expect(await screen.findByText(en.syncSources.nothingToDo)).toBeInTheDocument();
  });

  it("pauses and resumes without touching the repository (MF-30)", async () => {
    const calls = renderSyncSources();
    await screen.findByText("regional-datamodels");

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.pause }));

    await waitFor(() => {
      const pause = calls.find((call) => call.path.endsWith("/pause"));
      expect(pause?.body).toBe(JSON.stringify({ paused: true }));
    });
    expect(
      calls.some((call) => call.method !== "GET" && call.path.includes("/changes")),
    ).toBe(false);
  });

  it("a paused source offers Resume and no run at all", async () => {
    renderSyncSources({ ...PENDING, phase: "Paused", paused: true, mergeRequest: null });
    await screen.findByText("regional-datamodels");

    expect(await screen.findByRole("button", { name: en.syncSources.resume })).toBeEnabled();
    // Disabled with its reason (T-3850): aria-disabled, so the reason stays reachable.
    const syncNow = screen.getByRole("button", { name: en.syncSources.syncNow });
    expect(syncNow).toHaveAttribute("aria-disabled", "true");
    expect(syncNow).toHaveAccessibleDescription(en.syncSources.pausedReason);
  });

  it("asks before detaching, and answers with the merge request that removes it", async () => {
    const calls = renderSyncSources();
    await screen.findByText("regional-datamodels");

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.detach }));
    // Cancelling detaches nothing: the source is still attached after the dialog closes.
    const asking = await screen.findByRole("alertdialog");
    expect(asking).toHaveAccessibleDescription(expect.stringContaining("regional-datamodels"));
    await userEvent.click(within(asking).getByTestId("confirm-cancel"));
    expect(calls.some((call) => call.path.endsWith("/detach"))).toBe(false);

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.detach }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByTestId("confirm-accept"));

    await waitFor(() => {
      expect(calls.some((call) => call.path.endsWith("/detach"))).toBe(true);
    });
    // The page's notice among the Shell's own live regions (the jobs menu, T-3245).
    const notice = (await screen.findAllByRole("status")).find((region) =>
      region.textContent?.includes(en.syncSources.detached),
    );
    expect(notice).toBeDefined();
    if (!notice) return;
    expect(within(notice).getByRole("link", { name: new RegExp(`^${en.syncSources.review}`) })).toHaveAttribute(
      "href",
      "https://forge.example/pulls/9",
    );
  });

  it("warns when a restart would forget where each source stood", async () => {
    renderSyncSources({ ...PENDING, durable: false });
    expect(await screen.findByText(en.syncSources.notDurable)).toBeInTheDocument();
  });

  it("adds a source from the page the sidebar leads to (T-0790, MF-27)", async () => {
    const calls = renderSyncSources();

    // The sidebar has an entry, which is the only way a person finds this page.
    expect(await screen.findByRole("link", { name: en.nav.sync })).toBeInTheDocument();

    await userEvent.click(await screen.findByRole("button", { name: en.syncSources.add }));
    const dialog = await findFormPage();
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "regional-models");
    await userEvent.type(
      within(dialog).getByLabelText(/^Clone URL/),
      "https://git.region.sk/udp/datamodels.git",
    );
    await userEvent.clear(within(dialog).getByLabelText(/^Branch, tag or commit/));
    await userEvent.type(within(dialog).getByLabelText(/^Branch, tag or commit/), "main");
    // Checked first, as every kind's form is (PF-57): Propose opens on a fresh green verdict.
    await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
    const propose = within(dialog).getByRole("button", { name: en.syncSources.propose });
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    await userEvent.click(propose);

    await waitFor(() => {
      const posted = calls.find((call) => call.method === "POST" && call.path.endsWith("/syncsources"));
      expect(posted).toBeDefined();
      const body = JSON.parse(posted?.body ?? "{}") as {
        kind: string;
        metadata: { name: string };
        spec: { source: { git: { url: string; ref: string } }; schedule: { interval: string }; mode: string };
      };
      expect(body.kind).toBe("SyncSource");
      expect(body.metadata.name).toBe("regional-models");
      expect(body.spec.source.git.url).toBe("https://git.region.sk/udp/datamodels.git");
      expect(body.spec.source.git.ref).toBe("main");
      expect(body.spec.schedule.interval).toBe("6h");
      expect(body.spec.mode).toBe("mirror");
    });
  });
});
