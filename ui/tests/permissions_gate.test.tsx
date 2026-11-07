import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { allows } from "../src/api/permissions";
import type { Effective } from "../src/api/permissions";
import { expectDenied } from "./checks";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  email: "jana.kovacova@banskabystrica.sk",
  roles: [],
  groups: ["air-quality-team"],
};

const DEVELOPER = {
  project: "banskabystrica",
  bootstrap: false,
  grants: [
    {
      role: "pipeline-developer",
      binding: "developers",
      rule: { kinds: ["Pipeline", "DataSource"], verbs: ["propose"] },
    },
  ],
} as unknown as Effective;

const VIEWER: Effective = { project: "banskabystrica", bootstrap: false, grants: [] };

function renderPipelines(permissions: unknown) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me")) {
      return json(permissions);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("roles as code in the UI (T-0526, PF-50)", () => {
  it("a developer sees the propose control", async () => {
    const fetchMock = renderPipelines(DEVELOPER);
    // The page offers the same action twice when it has nothing to list: in the header and in
    // the empty state (T-1058). Both are guarded, so both are asserted.
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: en.pipelines.add }).length).toBeGreaterThan(0);
    });
    for (const control of screen.getAllByRole("button", { name: en.pipelines.add })) {
      expect(control).toBeEnabled();
    }
    expect(fetchMock.mock.calls.some((call) => (call[0] as Request).url.includes("/permissions/me"))).toBe(true);
  });

  it("a viewer with no binding sees the propose control disabled, with the reason (UI-44)", async () => {
    const fetchMock = renderPipelines(VIEWER);
    await waitFor(() => {
      expect(fetchMock.mock.calls.some((call) => (call[0] as Request).url.includes("/permissions/me"))).toBe(true);
    });
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: en.pipelines.title })).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: en.pipelines.add }).length).toBeGreaterThan(0);
    });
    // Every control the guard covers, not the first one it finds: a guard that closed the
    // header and left the empty state's call to action open would be the bug to catch.
    for (const control of screen.getAllByRole("button", { name: en.pipelines.add })) {
      expectDenied(
        control,
        "Disabled: your role does not permit 'propose' on 'Pipeline' in this project",
      );
    }
  });

  it("allows() reads kinds and verbs of every grant, lets bootstrap through and fails open without a document", () => {
    expect(allows(DEVELOPER, "Pipeline", "propose")).toBe(true);
    expect(allows(DEVELOPER, "Pipeline", "approve")).toBe(false);
    expect(allows(DEVELOPER, "Endpoint", "propose")).toBe(false);
    expect(allows(DEVELOPER, "*", "propose")).toBe(true);
    expect(allows(VIEWER, "*", "propose")).toBe(false);
    expect(allows({ ...VIEWER, bootstrap: true }, "Anything", "delete")).toBe(true);
    // No document yet: the control stays, the API answers 403 if it must.
    expect(allows(undefined, "Pipeline", "propose")).toBe(true);
  });
});
