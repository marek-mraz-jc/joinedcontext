// T-7589: SyncSourcesPage: a proposal that lands as a Change, pausing a source, a source with no origin.
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { digestOf } from "../src/api/digest";
import { SyncSourcesPage } from "../src/pages/sync/SyncSourcesPage";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "helsinki";
const PERMISSIONS = {
  project: PROJECT,
  bootstrap: false,
  grants: [{ role: "admin", binding: "admins", rule: { kinds: ["SyncSource"], verbs: ["read", "propose", "delete"] } }],
};

type Call = { method: string; path: string; body: string; dry: boolean };

function renderSync(items: unknown[], status: Record<string, unknown>): Call[] {
  const calls: Call[] = [];
  renderPage(<SyncSourcesPage project={PROJECT} />, {
    path: `/projects/${PROJECT}/syncsources`,
    answer: async (url, request) => {
      const body = request.method === "GET" ? "" : await request.clone().text();
      calls.push({ method: request.method, path: url.pathname, body, dry: url.searchParams.get("dryRun") === "All" });
      if (url.pathname.endsWith("/permissions/me")) return json(PERMISSIONS);
      if (url.pathname.endsWith("/status")) return json(status);
      if (url.pathname.endsWith("/pause")) return json({ ...status, paused: !status.paused });
      const draft = /\/drafts\/SyncSource\/([^/]+)$/.exec(url.pathname);
      if (request.method === "PUT" && draft) {
        const { manifest } = JSON.parse(body) as { manifest: unknown };
        return json({
          kind: "SyncSource",
          name: draft[1],
          project: PROJECT,
          manifest,
          touchedBy: "jana.kovacova",
          touchedKind: "person",
          updatedAt: new Date().toISOString(),
          verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(manifest) },
          version: 1,
        });
      }
      if (url.pathname.endsWith("/syncsources") && request.method === "POST") {
        if (calls[calls.length - 1].dry) return json({ valid: true });
        return json(
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "chg-4711" },
            status: { phase: "PendingApproval", lane: "red" },
          },
          202,
        );
      }
      if (url.pathname.endsWith("/syncsources")) return json(list(items));
      return undefined;
    },
  });
  return calls;
}

const GIT_SOURCE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "SyncSource",
  metadata: { name: "upstream", namespace: PROJECT },
  spec: { source: { git: { url: "https://git.example.sk/city/org" } }, schedule: { interval: "6h" }, mode: "mirror" },
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("sync sources: proposing and pausing", () => {
  it("proposes a new source and tells the person the Change that was opened, closing the form", async () => {
    const user = userEvent.setup();
    const calls = renderSync([], { phase: "Live", durable: true, paused: false });
    await user.click(await screen.findByRole("button", { name: en.syncSources.add }));
    const dialog = await screen.findByRole("dialog", { name: en.syncSources.dialog.title });
    await user.type(within(dialog).getByLabelText(/^Name/), "regional-models");
    await user.type(within(dialog).getByLabelText(/^Clone URL/), "https://git.region.sk/udp/datamodels.git");
    await user.click(within(dialog).getByRole("button", { name: en.form.check }));
    const propose = within(dialog).getByRole("button", { name: en.syncSources.propose });
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(propose);

    expect(await screen.findByText("chg-4711")).toBeInTheDocument();
    expect(screen.getByText(en.changes.accepted)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("dialog", { name: en.syncSources.dialog.title })).toBeNull());
    const posts = calls.filter((c) => c.method === "POST" && c.path.endsWith("/syncsources"));
    expect(posts.some((c) => c.dry)).toBe(true);
    const sent = posts.filter((c) => !c.dry).map((c) => JSON.parse(c.body) as { metadata: { name: string } });
    expect(sent.map((m) => m.metadata.name)).toEqual(["regional-models"]);
  });

  it("pauses a running source, asking the server to pause it", async () => {
    const user = userEvent.setup();
    const calls = renderSync([GIT_SOURCE], { phase: "Live", durable: true, paused: false });
    await user.click(await screen.findByRole("button", { name: en.syncSources.pause }));
    await waitFor(() => {
      const pause = calls.find((c) => c.path.endsWith("/pause"));
      expect(pause?.method).toBe("POST");
      expect(pause?.body).toBe(JSON.stringify({ paused: true }));
    });
  });

  it("resumes a paused source", async () => {
    const user = userEvent.setup();
    const calls = renderSync([GIT_SOURCE], { phase: "Paused", durable: true, paused: true });
    await user.click(await screen.findByRole("button", { name: en.syncSources.resume }));
    await waitFor(() => {
      expect(calls.find((c) => c.path.endsWith("/pause"))?.body).toBe(JSON.stringify({ paused: false }));
    });
  });

  it("shows a dash as the origin of a source that names none", async () => {
    renderSync([{ ...GIT_SOURCE, metadata: { name: "bare", namespace: PROJECT }, spec: {} }], { phase: "Live", durable: true, paused: false });
    const card = (await screen.findByText("bare")).closest("article") as HTMLElement;
    const origin = within(card).getByText(en.syncSources.origin);
    expect(origin.nextElementSibling).toHaveTextContent("—");
  });
});
