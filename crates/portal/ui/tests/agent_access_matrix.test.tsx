/**
 * T-4850, API/01 §52, AG-130…AG-132: an agent account's Access view. Each grant with its state,
 * the kind × verb matrix with the grant behind each cell or why it is refused, approve never, and
 * Revoke now behind a confirmation for a person who may revoke.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { AccountAccess, accessMatrix } from "../src/pages/access/AccountAccess";
import { fromServiceAccountEnvelope, toServiceAccountEnvelope } from "../src/pages/access/ServiceAccounts";

const ACTIVE = {
  index: 0,
  role: "pipeline-admin",
  scope: "project:helsinki",
  state: "active" as const,
  notAfter: "2026-11-01T00:00:00Z",
  rules: [{ kinds: ["Pipeline"], verbs: ["propose", "delete"] }],
};
const ENDED = {
  index: 1,
  role: "endpoint-editor",
  scope: "project:helsinki",
  state: "expired" as const,
  notAfter: "2026-10-01T00:00:00Z",
  rules: [{ kinds: ["Endpoint"], verbs: ["propose"] }],
};

function answer(mayRevoke: boolean) {
  return { account: "vendor-ci", project: "helsinki", delegates: false, mayPropose: true, mayRevoke, grants: [ACTIVE, ENDED] };
}

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

function stub(mayRevoke: boolean): Sent[] {
  const sent: Sent[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const text = request.method === "POST" ? await request.text() : "";
      sent.push({ method: request.method, path: url.pathname, body: text ? JSON.parse(text) : undefined });
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/serviceaccounts/vendor-ci/access")) return json(answer(mayRevoke));
      if (url.pathname.endsWith("/serviceaccounts/vendor-ci/revoke")) {
        return json(
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "chg-00000042", namespace: "helsinki" },
            status: { phase: "Deploying", lane: "red" },
          },
          202,
        );
      }
      return json({ items: [] });
    }),
  );
  return sent;
}

function show() {
  // The Change notice after a revoke links to the Change, so the view renders under a router.
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <AccountAccess project="helsinki" account="vendor-ci" />,
  });
  const router = createRouter({ routeTree: root.addChildren([page]) });
  window.history.pushState({}, "", "/");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
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

describe("accessMatrix (AG-130)", () => {
  it("names the grant behind each cell, the ended one behind a refusal, and never approve", () => {
    const rows = accessMatrix([ACTIVE, ENDED]);
    expect(rows.map((row) => row.kind)).toEqual(["Endpoint", "Pipeline"]);
    const pipeline = rows[1].cells;
    expect(pipeline.propose).toMatchObject({ allowed: true, sources: ["pipeline-admin · project:helsinki"] });
    expect(pipeline.read.allowed).toBe(true);
    expect(pipeline.approve).toMatchObject({ allowed: false, reason: { key: "access.agent.reason.neverApproves" } });
    const endpoint = rows[0].cells;
    expect(endpoint.propose).toMatchObject({ allowed: false, reason: { key: "access.agent.reason.ended" } });
    expect(endpoint.delete.reason?.key).toBe("access.agent.reason.noGrant");
  });
});

describe("AccountAccess (AG-130, AG-132)", () => {
  it("draws the grants and the matrix, and revokes one after the confirmation", async () => {
    const sent = stub(true);
    show();
    const table = await screen.findByRole("group", { name: "What vendor-ci may do, by kind" });
    const pipeline = within(table).getByRole("row", { name: /Pipeline/ });
    expect(within(pipeline).getAllByText("Allowed")).toHaveLength(3);
    expect(within(pipeline).getByText("An account never approves; a person does")).toBeInTheDocument();
    expect(screen.getByText("Expired")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Revoke endpoint-editor in project:helsinki now" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Revoke endpoint-editor from vendor-ci now?")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Revoke now" }));
    await waitFor(() => expect(sent.some((call) => call.path.endsWith("/revoke"))).toBe(true));
    expect(sent.find((call) => call.path.endsWith("/revoke"))?.body).toEqual({ index: 1, role: "endpoint-editor" });
  });

  it("offers no Revoke to a person who may not revoke", async () => {
    stub(false);
    show();
    await screen.findByRole("group", { name: "What vendor-ci may do, by kind" });
    expect(screen.queryByRole("button", { name: /Revoke/ })).not.toBeInTheDocument();
  });
});

describe("ServiceAccount form keeps a grant's window (AG-131)", () => {
  it("reads notAfter and notBefore and writes them back unchanged", () => {
    const stored = {
      metadata: { name: "vendor-ci" },
      spec: {
        owner: { user: "jana@hel.fi" },
        purpose: "ci",
        roles: [
          {
            role: "pipeline-admin",
            scope: { project: "helsinki" },
            validity: { notBefore: "2026-10-01T00:00:00Z", notAfter: "2026-11-01T00:00:00Z" },
          },
          { role: "space-reader", scope: { contextSpace: "air" } },
        ],
        credentials: [{ kind: "oauth-client", name: "main" }],
      },
    };
    const form = fromServiceAccountEnvelope(stored);
    expect(form.roles[0]).toMatchObject({ notAfter: "2026-11-01T00:00:00Z", notBefore: "2026-10-01T00:00:00Z" });
    const back = toServiceAccountEnvelope("helsinki", form, stored) as { spec: { roles: unknown[] } };
    expect(back.spec.roles).toEqual(stored.spec.roles);
  });
});
