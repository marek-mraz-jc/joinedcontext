/**
 * Opening a project from the sidebar (T-0870, PF-65, PF-66, UI-44): the control is always there,
 * disabled with the API's own reason when the organization does not let this caller open one,
 * and the name is checked while it is typed, before anything is sent.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/layout/NewProject.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { expectDenied, expectOpen } from "./checks";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: [],
};

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const REFUSED = "opening a project here needs propose on Project, which org-admin holds (PF-65)";

function renderShell(creation: { allowed: boolean; reason?: string }) {
  const posted: string[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const method = request?.method ?? init?.method ?? "GET";
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me")) {
      return json({ project: "banskabystrica", bootstrap: false, grants: [], projects: { creation } });
    }
    if (method === "POST" && path === "/api/v1/projects") {
      posted.push(request ? await request.clone().text() : String(init?.body ?? ""));
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-0000002a", namespace: "doprava" },
          status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2, update: 0, delete: 0 } },
        },
        202,
      );
    }
    return json(list([]));
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
  return { posted };
}

describe("the New project control in the sidebar", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is disabled with the API's own reason when the organization does not let this caller open a project", async () => {
    renderShell({ allowed: false, reason: REFUSED });

    // Refused, never hidden, and reachable so the reason can be read (UI-44, T-1758). It used
    // to be a hard-`disabled` button inside a bare `tabIndex={0}` span: an unnamed tab stop, a
    // `title` the disabled button swallowed, and a `role="tooltip"` nothing referenced.
    await waitFor(() => {
      expectDenied(screen.getByRole("button", { name: "New project" }), REFUSED);
    });
    const button = screen.getByRole("button", { name: "New project" });
    expect(button).toHaveAttribute("title", REFUSED);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("checks the name while it is typed and proposes the project as one change", async () => {
    const { posted } = renderShell({ allowed: true });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    const dialog = await screen.findByRole("dialog", { name: /Open a project/ });
    const open = within(dialog).getByRole("button", { name: "Open project" });
    // Nothing typed yet: nothing to send, and the button says so where it stands.
    expectDenied(open, "A usable name is needed to open a project.");

    const name = within(dialog).getByLabelText(/Name/);
    await user.type(name, "Doprava Mesta");
    expect(
      within(dialog).getByText(
        "Lowercase letters, digits and hyphens, starting and ending with a letter or a digit.",
      ),
    ).toBeTruthy();
    expectDenied(open);
    expect(posted).toHaveLength(0);

    await user.clear(name);
    await user.type(name, "doprava");
    expectOpen(open);
    await user.click(open);

    await screen.findByText(/chg-0000002a/);
    expect(posted).toHaveLength(1);
    expect(JSON.parse(posted[0])).toMatchObject({ name: "doprava" });
  });
});
