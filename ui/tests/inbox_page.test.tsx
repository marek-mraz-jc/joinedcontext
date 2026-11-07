// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/inbox/InboxPage.tsx, src/api/decision.ts.
/**
 * T-3273: one list of what waits for the signed-in person. Every change of every project they may
 * decide, each decided where it is listed with its diff; a Red-lane change asks for its name, a
 * rejection for its reason; a change the person may not decide is not offered; mentions are
 * listed; and the header counts what waits.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { InboxPage } from "../src/pages/inbox/InboxPage";
import { useDecisions } from "../src/api/decision";
import { renderHook } from "@testing-library/react";
import { AuthProvider } from "../src/auth/AuthProvider";
import { InRouter } from "./pageHarness";

const IDENTITY = { subject: "s", username: "jana", name: "Jana", email: "jana@hel.fi", roles: [] };

function change(project: string, name: string, kind: string, lane = "yellow", author = "marek@hel.fi") {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name, namespace: project },
    summary: { key: "change.summary.update", params: { kind, name: `${kind.toLowerCase()}-x`, fields: 1 } },
    author: { name: "Marek", email: author },
    createdAt: "2026-10-07T12:00:00Z",
    status: { lane, phase: "PendingApproval", plan: { update: 1 } },
    planFields: [{ path: "spec.audience", from: "organization", to: "project-list" }],
  };
}

const FORGE_DOWN = "The Git repository did not answer the Portal. Try again in a minute.";

interface Seen {
  writes: { path: string; body: unknown }[];
}

function show(seen: Seen, unavailable?: string) {
  window.history.pushState({}, "", "/projects/helsinki/inbox");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (request.method !== "GET") {
        const body = request.method === "POST" ? await request.text() : "";
        seen.writes.push({ path: url.pathname, body: body ? JSON.parse(body) : undefined });
        const project = url.pathname.split("/")[4];
        const id = url.pathname.split("/")[6];
        const phase = url.pathname.endsWith("/reject") ? "Rejected" : "Deploying";
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: id, namespace: project }, status: { lane: "yellow", phase, plan: { update: 1 } } }, 202);
      }
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }, { name: "ovzdusie" }] });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "x", bootstrap: false, grants: [{ role: "approver", binding: "b", scope: "project", rule: { kinds: ["Endpoint", "Policy"], verbs: ["approve"] } }] });
      }
      if (unavailable && url.pathname === `/api/v1/projects/${unavailable}/changes`) {
        return new Response(JSON.stringify({ type: "about:blank", title: "Service Unavailable", status: 503, detail: FORGE_DOWN }), {
          status: 503,
          headers: { "Content-Type": "application/problem+json" },
        });
      }
      if (url.pathname === "/api/v1/projects/helsinki/changes") {
        return json({ items: [change("helsinki", "chg-endpoint", "Endpoint"), change("helsinki", "chg-pipeline", "Pipeline")] });
      }
      if (url.pathname === "/api/v1/projects/ovzdusie/changes") return json({ items: [change("ovzdusie", "chg-policy", "Policy", "red")] });
      if (url.pathname.includes("/changes/")) {
        const [, , , , project, , id] = url.pathname.split("/");
        return json(change(project, id, "Endpoint"));
      }
      if (url.pathname === "/api/v1/notifications") {
        return json({
          unread: 2,
          items: [
            { id: 7, kind: "mention", read: false, project: "helsinki", space: "ovzdusie", authorName: "Peter", excerpt: "@jana look at pm10", createdAt: "2026-10-07T12:00:00Z" },
            // T-5979: a change a connector opened, for the approver.
            { id: 8, kind: "change.opened", read: false, project: "helsinki", space: "", urn: "cr-12", authorName: "Marek", excerpt: "doprava-in-claude", link: "/projects/helsinki/approvals/cr-12", createdAt: "2026-10-07T12:01:00Z" },
          ],
        });
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const item = async (text: RegExp) => (await screen.findByText(text)).closest("li") as HTMLElement;

describe("the inbox (T-3273)", () => {
  it("lists what the person may decide in every project, and not what they may not", async () => {
    const seen = { writes: [] };
    show(seen);
    const decisions = await screen.findByTestId("inbox-decisions");
    await waitFor(() => expect(within(decisions).getAllByRole("listitem")).toHaveLength(2));
    expect(decisions).toHaveTextContent("proposed by Marek in helsinki");
    expect(decisions).toHaveTextContent("proposed by Marek in ovzdusie");
    expect(decisions).not.toHaveTextContent("pipeline-x");
    expect(screen.getByRole("link", { name: /Peter mentioned you/ })).toBeInTheDocument();
  });

  it("lists a connector event apart from the mentions, linking its approval page (T-5979)", async () => {
    show({ writes: [] });
    const events = await screen.findByTestId("inbox-connector-events");
    const link = within(events).getByRole("link", { name: /Marek opened change cr-12 in helsinki through connector doprava-in-claude/ });
    expect(link).toHaveAttribute("href", "/projects/helsinki/approvals/cr-12");
    expect(within(events).queryByText(/mentioned you/)).toBeNull();
  });

  it("approves a change with its diff open, without leaving the page", async () => {
    const seen: Seen = { writes: [] };
    show(seen);
    const endpoint = await item(/Endpoint "endpoint-x"/);
    await userEvent.click(within(endpoint).getByRole("button", { name: en.inbox.showDiff }));
    expect(await within(endpoint).findByText("organization")).toBeInTheDocument();
    await userEvent.click(within(endpoint).getByRole("button", { name: en.approvals.approve }));
    await waitFor(() => expect(seen.writes).toEqual([{ path: "/api/v1/projects/helsinki/changes/chg-endpoint/approve", body: undefined }]));
    expect(await within(endpoint).findByRole("status")).toHaveTextContent(en.inbox.decided.approved);
  });

  it("asks for a Red-lane change's name before approving it, and for a reason before rejecting", async () => {
    const seen: Seen = { writes: [] };
    show(seen);
    const policy = await item(/Policy "policy-x"/);
    const approve = within(policy).getByRole("button", { name: en.approvals.approve });
    expect(approve).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(policy).getByLabelText(new RegExp(en.approvals.confirmPrompt)), "policy-x");
    expect(approve).not.toHaveAttribute("aria-disabled", "true");
    await userEvent.click(within(policy).getByRole("button", { name: en.approvals.reject }));
    const confirmReject = within(policy).getByRole("button", { name: en.approvals.rejectConfirm });
    expect(confirmReject).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(policy).getByLabelText(new RegExp(en.approvals.rejectReason)), "too wide");
    await userEvent.click(confirmReject);
    await waitFor(() =>
      expect(seen.writes).toEqual([{ path: "/api/v1/projects/ovzdusie/changes/chg-policy/reject", body: { reason: "too wide" } }]),
    );
  });

  it("counts what waits in the header and leads to the inbox", async () => {
    show({ writes: [] });
    await screen.findByTestId("inbox-decisions");
    // One decidable change in the project in hand, one unread mention, one connector event.
    const trigger = await screen.findByRole("button", { name: "Notifications, 3 unread" });
    await userEvent.click(trigger);
    expect(await screen.findByRole("menuitem", { name: "Open your inbox (3 waiting)" })).toBeInTheDocument();
  });

  it("says nothing waits when no change is the person's to decide, and decides only pending ones", async () => {
    show({ writes: [] });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { result } = renderHook(() => useDecisions("ovzdusie"), {
      wrapper: ({ children }) => (
        <QueryClientProvider client={client}>
          <AuthProvider>{children}</AuthProvider>
        </QueryClientProvider>
      ),
    });
    await waitFor(() => expect(result.current.map((c) => c.metadata.name)).toEqual(["chg-policy"]));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <InRouter>
              <InboxPage project="nowhere" />
            </InRouter>
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect((await screen.findAllByText(en.inbox.noMentions)).length).toBeGreaterThan(0);
  });

  it("says a project's changes did not load instead of claiming nothing waits (T-4966)", async () => {
    show({ writes: [] }, "helsinki");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The changes of helsinki did not load");
    expect(alert).toHaveTextContent(FORGE_DOWN);
    const decisions = screen.getByTestId("inbox-decisions");
    // The other project still lists what waits, and the list is not empty, so no "Nothing waits".
    await waitFor(() => expect(decisions).toHaveTextContent("proposed by Marek in ovzdusie"));
    expect(decisions).not.toBeEmptyDOMElement();
  });

  it("shows how to approve only while something waits, never to a person with nothing to decide (T-4165)", async () => {
    show({ writes: [] });
    const hint = await screen.findByText(en.inbox.decisionsHint);
    // jsdom draws no CSS: the hint hides itself when the decisions list before it is empty.
    expect(hint).toHaveClass("peer-empty:hidden");
    expect(screen.getByTestId("inbox-decisions")).toHaveClass("peer");
  });
});

