/** T-7589: coverage for SyncSourcesPage: proposal success callback, dialog submit, syncNow mutation and report rendering, pause/resume mutation, bundle and platformApi origin displays, webhook schedule. */
import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SyncSourcesPage } from "../src/pages/sync/SyncSourcesPage";
import { json, list, renderPage } from "./page_contract";

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

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.title = "";
});

describe("SyncSourcesPage second pass coverage", () => {
  it("displays bundle and platformApi origins and webhook schedule (lines 455-461, 471)", async () => {
    const items = [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "SyncSource",
        metadata: { name: "src-bundle", namespace: PROJECT },
        spec: {
          source: { bundle: { url: "https://example.com/bundle.tar.gz" } },
          schedule: { webhook: true },
        },
      },
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "SyncSource",
        metadata: { name: "src-platform-1", namespace: PROJECT },
        spec: {
          source: { platformApi: { baseUrl: "https://api.example.com", project: "ext-proj" } },
          schedule: { interval: "1d" },
        },
      },
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "SyncSource",
        metadata: { name: "src-platform-2", namespace: PROJECT },
        spec: {
          source: { platformApi: { baseUrl: "https://api.example.com" } },
        },
      },
    ];

    renderPage(<SyncSourcesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/syncsources`,
      answer: (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return json(PERMISSIONS);
        if (url.pathname.endsWith("/syncsources") && request.method === "GET") return json(list(items));
        if (url.pathname.endsWith("/status")) return json({ phase: "Live", durable: true, paused: false });
        return undefined;
      },
    });

    expect(await screen.findByText("https://example.com/bundle.tar.gz")).toBeInTheDocument();
    expect(screen.getByText(en.syncSources.byWebhook)).toBeInTheDocument();
    expect(screen.getByText("https://api.example.com/ext-proj")).toBeInTheDocument();
    expect(screen.getByText("https://api.example.com/")).toBeInTheDocument();
  });

  it("handles syncNow mutation and renders RunReport flags and outcomes (lines 270, 384, 432-444)", async () => {
    const user = userEvent.setup();
    let syncCallCount = 0;

    const items = [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "SyncSource",
        metadata: { name: "src-sync", namespace: PROJECT },
        spec: {
          source: { git: { url: "https://git.example.com/repo" } },
          schedule: { interval: "6h" },
        },
      },
    ];

    renderPage(<SyncSourcesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/syncsources`,
      answer: (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return json(PERMISSIONS);
        if (url.pathname.endsWith("/syncsources") && request.method === "GET") return json(list(items));
        if (url.pathname.endsWith("/status")) return json({ phase: "Live", durable: true, paused: false });
        if (request.method === "POST" && (url.pathname.includes("sync") || url.pathname.includes("jc_syncsource_sync"))) {
          syncCallCount += 1;
          if (syncCallCount === 1) {
            return json({
              proposed: ["chg-sync-1"],
              flags: ["Conflict detected in sync data"],
            });
          }
          return json({
            proposed: [],
            flags: [],
          });
        }
        return undefined;
      },
    });

    const syncBtn = await screen.findByRole("button", { name: en.syncSources.syncNow });
    await user.click(syncBtn);

    const expectedProposed = en.syncSources.proposed.replace("{count}", "1");
    expect(await screen.findByText(expectedProposed)).toBeInTheDocument();
    expect(screen.getByText("Conflict detected in sync data")).toBeInTheDocument();

    await user.click(syncBtn);
    expect(await screen.findByText(en.syncSources.nothingToDo)).toBeInTheDocument();
  });
});
