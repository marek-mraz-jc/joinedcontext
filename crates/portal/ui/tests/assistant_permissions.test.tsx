/**
 * The assistant page refuses what the person's role does not permit (T-1584; UI-44, PF-50).
 *
 * Every other page's write controls go through `PermissionGuard`; the assistant page's did not.
 * A viewer of the project — no grant at all — was offered `Start`, `Continue` and the agent
 * profile's `Edit`, and met the refusal only as a 403 after pressing them, with nothing in the
 * page saying which verb on which kind they lack. The live walk over every page found it
 * (`ui/e2e/live/viewer-refused.spec.ts`), and the door was never the hole: `create_run` checks
 * `App`/`propose` and the profile block is proposed as the org's `AgentProfile`.
 *
 * Both states of the guard are asserted, because a guard that disabled the control for everyone
 * would pass a test that only looked at the viewer.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import type { Effective } from "../src/api/permissions";
import { expectDenied } from "./checks";

const PROJECT = "banskabystrica";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: [], groups: [] };

/** A person with no grant in the project: they read it and change nothing. */
const VIEWER: Effective = { project: PROJECT, bootstrap: false, grants: [] };

/** A person who may propose an App, which is what a run writes, and the profile block. */
const BUILDER = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      role: "app-builder",
      binding: "builders",
      rule: { kinds: ["App", "AgentProfile"], verbs: ["propose"] },
    },
  ],
} as unknown as Effective;

/** One conversation in a terminal state, so the page offers `Continue` beside it. */
const ENDED_CONVERSATION = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "otazka",
  endpointName: "ovzdusie-public",
  appClass: "static",
  visibility: "project",
  kind: "conversation",
  prompt: "How many stations are there?",
  status: "published",
  steps: 2,
  tokensUsed: 900,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

const PROFILE = {
  name: "portal-assistant",
  title: "Portal assistant",
  role: "assistant",
  operations: [],
  egressHosts: [],
  access: null,
};

function renderAssistant(permissions: unknown) {
  const json = (body: unknown, status = 200) =>
    Promise.resolve(
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
    );
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = new URL((input as Request).url).pathname;
      if (path.endsWith("/auth/me")) {
        return json(IDENTITY);
      }
      if (path.endsWith("/permissions/me")) {
        return json(permissions);
      }
      if (path.endsWith("/agent-runs")) {
        return json({ items: [ENDED_CONVERSATION] });
      }
      if (path.endsWith("/assistant/access")) {
        return json({ items: [PROFILE] });
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the assistant page's write controls carry the refusal (T-1584, UI-44)", () => {
  it("a viewer meets every control disabled, and the reason names the verb and the kind", async () => {
    renderAssistant(VIEWER);

    // The wait is for `permissions/me`: until it arrives the guard lets the control through on
    // purpose, so an assertion on the first render would pass whether the guard were there or not.
    await waitFor(() =>
      expect(
        screen.getAllByRole("button").map((node) => node.getAttribute("title") ?? ""),
      ).toContain("Only an organization administrator can add or change agent profiles."),
    );

    for (const label of [en.assistantPage.continue, en.assistantPage.access.edit]) {
      expectDenied(screen.getByRole("button", { name: label }));
    }
  });

  it("a person who may propose an App is offered the controls, with no refusal on them", async () => {
    renderAssistant(BUILDER);

    const continued = await waitFor(() =>
      screen.getByRole("button", { name: en.assistantPage.continue }),
    );
    expect(continued).toBeEnabled();
    expect(screen.getByRole("button", { name: en.assistantPage.access.edit })).toBeEnabled();
    // The guard hands the control nothing when the verb is permitted.
    expect(screen.queryAllByRole("button").filter((node) => node.hasAttribute("aria-disabled"))).toEqual(
      [],
    );
  });
});
