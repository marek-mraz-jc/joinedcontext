// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/routes/ApprovalDetailPage.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { expectDenied } from "./checks";
import { ApprovalDetailPage } from "../src/routes/ApprovalDetailPage";
import { AuthProvider } from "../src/auth/AuthProvider";
import { expectNoAxeViolations, problem, renderPage } from "./page_contract";

const APPROVER = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

function proposal(over: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
    summary: {
      key: "change.summary.update",
      params: { kind: "Endpoint", name: "air-quality", fields: 2 },
    },
    author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
    createdAt: "2026-03-03T12:00:00Z",
    status: {
      lane: "yellow",
      phase: "PendingApproval",
      plan: { update: 1 },
      mergeRequest: "https://gitea.example/city/city-config/pulls/7",
    },
    planFields: [{ path: "spec.audience", from: "public", to: "internal" }],
    ...over,
  };
}

const DEPLOYING = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "Deploying", plan: { update: 1 } },
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function renderDetail(
  options: {
    identity?: unknown;
    change?: Record<string, unknown>;
    permissions?: unknown;
    // The change the API serves once it was approved, for a page that polls through Deploying.
    afterApproval?: Record<string, unknown>;
    // The live Endpoint the change edits, for the publication preview (T-4847).
    endpoint?: Record<string, unknown>;
  } = {},
) {
  const change = options.change ?? proposal();
  let approved = false;
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    if (request.url.includes("/auth/me")) {
      return Promise.resolve(json(options.identity ?? APPROVER));
    }
    // Roles as code (T-0526): the approver's grant comes from the bindings, not the token.
    if (request.url.includes("/permissions/me") && options.permissions) {
      return Promise.resolve(json(options.permissions));
    }
    if (request.url.includes("/permissions/me")) {
      const identity = (options.identity ?? APPROVER) as { roles?: string[] };
      const approver = identity.roles?.includes("portal-approver") ?? false;
      return Promise.resolve(
        json({ project: "banskabystrica", bootstrap: approver, grants: [] }),
      );
    }
    if (options.endpoint && request.url.includes("/endpoints/air-quality")) {
      return Promise.resolve(json(options.endpoint));
    }
    if (request.method === "POST") {
      approved = true;
      return Promise.resolve(json(DEPLOYING, 202));
    }
    return Promise.resolve(json(approved && options.afterApproval ? options.afterApproval : change));
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
  return fetchMock;
}

function posts(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method === "POST");
}

describe("approval actions", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    window.history.pushState({}, "", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // AG-137 (T-5609): a Change an AI client proposed over MCP says so to the approver, and one a
  // person proposed carries no such badge.
  it("tells the approver a change came through an AI client over MCP", async () => {
    renderDetail({
      change: proposal({
        status: {
          ...proposal().status,
          proposedVia: { channel: "mcp", client: "mcp-mobile" },
        },
      }),
    });
    const badge = await screen.findByTestId("change-proposed-via");
    expect(badge).toHaveTextContent("Proposed through mcp-mobile (MCP)");
    expect(
      screen.getByText(
        en.approvals.proposedVia.hint
          .replace("{client}", "mcp-mobile")
          .replace("{author}", "Marek Mráz"),
      ),
    ).toBeInTheDocument();
  });

  // T-5973: a change a connector opened names the connector and the plan its diff came from.
  it("names the connector and the plan of a change a connector applied", async () => {
    renderDetail({
      change: proposal({
        status: {
          ...proposal().status,
          proposedVia: {
            channel: "mcp",
            client: "claude-code",
            connector: "abcdefghijklmnopqrstuvwxyz",
            connectorName: "doprava-in-claude",
            plan: "p2345234523452345234523452",
          },
        },
      }),
    });
    const badge = await screen.findByTestId("change-proposed-via");
    expect(badge).toHaveTextContent("Via Claude Code connector doprava-in-claude");
    expect(screen.getByTestId("change-proposed-plan")).toHaveTextContent(
      "Plan p2345234523452345234523452",
    );
  });

  it("shows no channel badge on a change the person proposed", async () => {
    renderDetail();
    await screen.findByText("Marek Mráz");
    expect(screen.queryByTestId("change-proposed-via")).toBeNull();
  });

  // MF-48: a change that names what another creates says which, and a rejected one flags it.
  it("says which change it waits on, and flags one that was rejected", async () => {
    renderDetail({
      change: proposal({
        waitsOn: [
          { name: "chg-00000007", phase: "PendingApproval" },
          { name: "chg-00000009", phase: "Rejected" },
        ],
      }),
    });
    expect(
      await screen.findByText(en.approvals.waitsOn.pending.replace("{change}", "chg-00000007")),
    ).toBeInTheDocument();
    const rejected = screen.getByText(en.approvals.waitsOn.rejected.replace("{change}", "chg-00000009"));
    expect(rejected.closest("[role=status]")?.className).toContain("danger");
  });

  // T-4847, EP-142: the approver reads the CKAN page the publication becomes, beside the diff.
  it("previews the CKAN page a public publication becomes and asks for the name typed back", async () => {
    const files = [
      {
        path: "projects/banskabystrica/endpoints/air-quality.yaml",
        kind: "Endpoint",
        operation: "Update",
        lane: "red",
        fields: [
          { path: "spec.audience", from: "organization", to: "public" },
          { path: "metadata.title.en", from: "Air", to: "Air quality in the city" },
          { path: "spec.catalog.license", to: "CC0" },
          { path: "spec.catalog.keywords.en[0]", to: "pm10" },
          { path: "spec.publish.ckan.instanceRef", to: "bbsk" },
        ],
      },
    ];
    const fetchMock = renderDetail({
      change: proposal({ files, status: { lane: "red", phase: "PendingApproval", plan: { update: 1 } } }),
      endpoint: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Endpoint",
        metadata: { name: "air-quality", namespace: "banskabystrica", title: { en: "Air" } },
        spec: { audience: "organization", catalog: { themes: ["ENVI"] } },
      },
    });
    const preview = await screen.findByRole("region", { name: "The CKAN page of air-quality after the approval" });
    expect(within(preview).getByRole("heading", { name: "Air quality in the city" })).toBeInTheDocument();
    expect(within(preview).getByText("Creative Commons Zero 1.0")).toBeInTheDocument();
    expect(within(preview).getByText("Environment")).toBeInTheDocument();
    expect(within(preview).getByText("pm10")).toBeInTheDocument();
    expect(within(preview).queryByText(/Table in CKAN/)).toBeNull();
    expect(screen.getByText(en.approvals.publicationPublic)).toBeInTheDocument();

    // Red lane: approve stays shut until the name is typed back, and nothing is posted.
    const approve = screen.getByRole("button", { name: en.approvals.approve });
    expect(approve).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(approve);
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("posts the approval to the change of the route, with the CSRF token", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.approve }));

    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    const request = posts(fetchMock)[0];
    expect(request.url).toContain(
      "/api/v1/projects/banskabystrica/changes/chg-1a2b3c4d/approve",
    );
    expect(request.headers.get("x-csrf-token")).toBe("csrf-token-value");
  });

  it("says the change is being deployed, then that it is applied, with a link to its list", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const files = [
        { path: "projects/banskabystrica/endpoints/air-quality.yaml", kind: "Endpoint", operation: "Update", lane: "yellow" },
      ];
      renderDetail({
        change: proposal({ files }),
        afterApproval: proposal({ files, status: { lane: "yellow", phase: "Applied", plan: { update: 1 } } }),
      });
      // Nothing to say before anyone acted.
      await screen.findByRole("button", { name: en.approvals.approve });
      expect(screen.queryByText(/Change approved/)).toBeNull();

      await userEvent.click(screen.getByRole("button", { name: en.approvals.approve }));
      expect(await screen.findByText("Change approved. The Endpoint is being deployed.")).toBeInTheDocument();

      await vi.advanceTimersByTimeAsync(5_000);
      expect(await screen.findByText(en.approvals.approvedLive)).toBeInTheDocument();
      expect(screen.getByRole("link", { name: "Open the Endpoint list" })).toHaveAttribute(
        "href",
        "/projects/banskabystrica/endpoints",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("says nothing when the change was applied before the page opened", async () => {
    renderDetail({ change: proposal({ status: { lane: "yellow", phase: "Applied", plan: { update: 1 } } }) });
    await screen.findByText(en.phase.applied);
    expect(screen.queryByRole("status", { name: /Change approved/ })).toBeNull();
    expect(screen.queryByText(en.approvals.approvedLive)).toBeNull();
  });

  it("moves the status chip to Deploying once the merge is accepted", async () => {
    renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.approve }));

    expect(await screen.findByText(en.phase.deploying)).toBeInTheDocument();
    // Nothing is left to decide, so no decision is offered.
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: en.approvals.approve })).not.toBeInTheDocument(),
    );
  });

  it("asks why before it rejects, and sends the reason to the reject endpoint", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.reject }));

    const dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    expect(posts(fetchMock)).toHaveLength(0);
    await userEvent.type(within(dialog).getByLabelText(en.approvals.rejectReason), "  The slug is wrong  ");
    await userEvent.click(within(dialog).getByRole("button", { name: en.approvals.rejectConfirm }));

    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(posts(fetchMock)[0].url).toContain(
      "/api/v1/projects/banskabystrica/changes/chg-1a2b3c4d/reject",
    );
    await expect(posts(fetchMock)[0].clone().json()).resolves.toEqual({ reason: "The slug is wrong" });
  });

  it("cannot reject without a reason, and Escape or Cancel rejects nothing", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.reject }));
    const dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    const confirm = within(dialog).getByRole("button", { name: en.approvals.rejectConfirm });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(dialog).getByLabelText(en.approvals.rejectReason), "   ");
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: en.approvals.reject }));
    const again = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    await userEvent.click(within(again).getAllByRole("button", { name: en.approvals.rejectCancel })[0]);
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("sends a reason with Unicode and a line break as typed", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.reject }));
    const dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    await userEvent.type(within(dialog).getByLabelText(en.approvals.rejectReason), "Zlý slug{Enter}už existuje – µg/m³");
    await userEvent.click(within(dialog).getByRole("button", { name: en.approvals.rejectConfirm }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    await expect(posts(fetchMock)[0].clone().json()).resolves.toEqual({ reason: "Zlý slug\nuž existuje – µg/m³" });
  });

  it("disables both actions without the approver role, and says why", async () => {
    const fetchMock = renderDetail({
      identity: { ...APPROVER, roles: ["portal-viewer"] },
    });

    // Refused, and still reachable: a hard-disabled button leaves the tab order, and the reason
    // written for it can then never be read (T-1743, UI-44).
    // Each control carries its own reason rather than a paragraph beside them all: by pointer
    // the tooltip, by keyboard the description, and the control stays in the tab order.
    expectDenied(await screen.findByRole("button", { name: en.approvals.approve }), en.approvals.needsRole);
    expectDenied(screen.getByRole("button", { name: en.approvals.reject }), en.approvals.needsRole);
    const approve = screen.getByRole("button", { name: en.approvals.approve });
    expect(approve).toHaveAttribute("title", en.approvals.needsRole);

    await userEvent.click(screen.getByRole("button", { name: en.approvals.approve }));
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("keeps an author from approving their own proposal (AG-11)", async () => {
    const fetchMock = renderDetail({
      change: proposal({ author: { name: "Jana Kováčová", email: APPROVER.email } }),
    });

    expectDenied(
      await screen.findByRole("button", { name: en.approvals.approve }),
      en.approvals.ownProposal,
    );
    // Withdrawing is still theirs: closing never merges (T-6535).
    expect(screen.getByRole("button", { name: en.approvals.withdraw })).toBeEnabled();
    expect(screen.queryByRole("button", { name: en.approvals.reject })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: en.approvals.approve }));
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("lets the proposer withdraw their own model change without approve, and approve stays shut (T-6535)", async () => {
    const fetchMock = renderDetail({
      identity: { ...APPROVER, roles: ["portal-viewer"] },
      change: proposal({
        author: { name: "Jana Kováčová", email: APPROVER.email },
        summary: { key: "change.summary.update", params: { kind: "DataModel", name: "air", fields: 1 } },
        files: [{ path: "projects/banskabystrica/spaces/air/datamodels/air.yaml", kind: "DataModel", operation: "Update", lane: "yellow", fields: [] }],
      }),
    });

    expectDenied(await screen.findByRole("button", { name: en.approvals.approve }), en.approvals.needsRole);
    const withdraw = screen.getByRole("button", { name: en.approvals.withdraw });
    expect(withdraw).toBeEnabled();
    await userEvent.click(withdraw);
    const dialog = await screen.findByRole("dialog", { name: en.approvals.withdrawTitle });
    expect(within(dialog).getByText(en.approvals.withdrawReasonHint)).toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText(en.approvals.withdrawReason), "Wrong version");
    await userEvent.click(within(dialog).getByRole("button", { name: en.approvals.withdrawConfirm }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(posts(fetchMock)[0].url).toContain("/changes/chg-1a2b3c4d/reject");
    await expect(posts(fetchMock)[0].clone().json()).resolves.toEqual({ reason: "Wrong version" });
    expect(await screen.findByText(en.approvals.withdrawn)).toBeInTheDocument();
  });

  it("lets an administrator of the kind approve their own proposal, and says so (PF-58)", async () => {
    const fetchMock = renderDetail({
      change: proposal({ author: { name: "Jana Kováčová", email: APPROVER.email } }),
      permissions: {
        project: "banskabystrica",
        bootstrap: false,
        grants: [
          {
            role: "org-admin",
            binding: "admins",
            rule: { kinds: ["Endpoint"], verbs: ["propose", "approve", "delete"] },
          },
        ],
      },
    });

    const approve = await screen.findByRole("button", { name: en.approvals.approve });
    await waitFor(() => expect(approve).toBeEnabled());
    expect(
      screen.getByText(i18n.t("approvals.ownAsAdministrator", { kind: "Endpoint" })),
    ).toBeInTheDocument();
    await userEvent.click(approve);
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(posts(fetchMock)[0].url).toContain("/changes/chg-1a2b3c4d/approve");
  });

  it("keeps an author who may approve but not delete the kind from approving their own proposal", async () => {
    const fetchMock = renderDetail({
      change: proposal({ author: { name: "Jana Kováčová", email: APPROVER.email } }),
      permissions: {
        project: "banskabystrica",
        bootstrap: false,
        grants: [
          {
            role: "steward",
            binding: "stewards",
            rule: { kinds: ["Endpoint"], verbs: ["propose", "approve"] },
          },
        ],
      },
    });

    expectDenied(
      await screen.findByRole("button", { name: en.approvals.approve }),
      en.approvals.ownProposal,
    );
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("does_not_steal_the_focus_on_a_red_lane_approval_and_still_labels_the_name_to_type_back", async () => {
    // T-1394 put the focus here on arrival; the UI rules took it back out (T-1726). A red-lane
    // change is the one an approver has to READ before confirming, and dropping the focus past
    // the diff is exactly what a screen reader then skips. The field is labelled, described by
    // the name to type, and one Tab away.
    renderDetail({ change: proposal({ status: { lane: "red", phase: "PendingApproval", plan: { update: 1 } } }) });
    const input = await screen.findByRole("textbox", { name: en.approvals.confirmLabel });
    expect(input).not.toHaveFocus();
    expect(document.activeElement).toBe(document.body);
    expect(input).toHaveAccessibleDescription(new RegExp("air-quality"));
    await userEvent.tab();
    expect(document.activeElement).not.toBe(document.body);
  });

  it("holds a red-lane approval until the resource name is typed back", async () => {
    const fetchMock = renderDetail({
      change: proposal({
        status: { lane: "red", phase: "PendingApproval", plan: { delete: 1 } },
      }),
    });

    const held = en.approvals.confirmFirst.replace("{name}", "air-quality");
    expectDenied(await screen.findByRole("button", { name: en.approvals.approve }), held);
    expect(screen.getByRole("button", { name: en.approvals.reject })).toBeEnabled();

    expect(screen.getByRole("button", { name: en.approvals.approve })).toHaveAttribute("title", held);
    const confirm = screen.getByLabelText(en.approvals.confirmLabel);
    await userEvent.type(confirm, "air-qualit");
    expectDenied(screen.getByRole("button", { name: en.approvals.approve }), held);

    await userEvent.type(confirm, "y");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: en.approvals.approve })).toBeEnabled(),
    );

    await userEvent.click(screen.getByRole("button", { name: en.approvals.approve }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    await expect(posts(fetchMock)[0].clone().json()).resolves.toEqual({ confirm: "air-quality" });
  });
});

describe("the files of a bundle (T-0861)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
    window.history.pushState({}, "", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every file of the merge request, the strictest lane first", async () => {
    renderDetail({
      change: proposal({
        fileCount: 2,
        files: [
          {
            path: "projects/banskabystrica/pipelines/aq.yaml",
            kind: "Pipeline",
            operation: "Create",
            lane: "green",
          },
          {
            path: "users/assignments/mallory-admin.yaml",
            kind: "RoleBinding",
            operation: "Create",
            lane: "red",
          },
        ],
      }),
    });

    const rows = await screen.findAllByTestId("change-file");
    expect(rows).toHaveLength(2);
    // The one that decides the confirmation is read first.
    expect(rows[0]).toHaveTextContent("RoleBinding");
    expect(rows[0]).toHaveTextContent("users/assignments/mallory-admin.yaml");
    expect(rows[0]).toHaveTextContent(en.lane.red);
    expect(rows[1]).toHaveTextContent("Pipeline");
    expect(rows[1]).toHaveTextContent(en.approvals.diffAdded);
  });

  it("shows the field diff of the file an approver clicks, and the headline again on a second click", async () => {
    renderDetail({
      change: proposal({
        files: [
          {
            path: "users/assignments/mallory-admin.yaml",
            kind: "RoleBinding",
            operation: "Update",
            lane: "red",
            fields: [{ path: "spec.role", from: "viewer", to: "admin" }],
          },
          { path: "README.md", operation: "Update", lane: "green" },
        ],
      }),
    });

    const heading = await screen.findByRole("heading", { name: en.approvals.diffType });
    expect(screen.getByTitle("spec.audience")).toBeInTheDocument();
    // A file without a manifest has nothing to diff field by field, so it is not a button.
    expect(screen.queryByRole("button", { name: "README.md" })).toBeNull();

    const file = screen.getByRole("button", { name: "users/assignments/mallory-admin.yaml" });
    fireEvent.click(file);
    expect(heading).toHaveTextContent("users/assignments/mallory-admin.yaml");
    expect(screen.getByTitle("spec.role")).toBeInTheDocument();
    expect(screen.queryByTitle("spec.audience")).toBeNull();
    expect(file).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(file);
    expect(heading).toHaveTextContent(en.approvals.diffType);
    expect(screen.getByTitle("spec.audience")).toBeInTheDocument();
  });

  it("shows no file section for a change the API listed no files for", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: en.approvals.diffType });
    expect(screen.queryAllByTestId("change-file")).toHaveLength(0);
  });
});

describe("a change brought back from a copy (UI-63)", () => {
  it("says which copy it came from and whose it was", async () => {
    renderDetail({ change: proposal({ workspace: "air-v2" }) });
    const line = await screen.findByTestId("change-workspace");
    expect(line.textContent).toContain("air-v2");
    expect(line.textContent).toContain("Marek Mráz");
  });

  it("says nothing of a copy for an ordinary change", async () => {
    renderDetail({ change: proposal() });
    await screen.findByText("Marek Mráz");
    expect(screen.queryByTestId("change-workspace")).toBeNull();
  });
});

describe("the CI of the project repository (CC-90, T-3433)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  it("says in words that the head commit's CI failed", async () => {
    renderDetail({ change: proposal({ ci: "failure" }) });
    const line = await screen.findByTestId("change-ci");
    expect(line.textContent).toContain(en.approvals.ci.label);
    expect(line.textContent).toContain(en.approvals.ci.failure);
  });

  it("says nothing of CI while no check has reported", async () => {
    renderDetail({ change: proposal() });
    await screen.findByText("Marek Mráz");
    expect(screen.queryByTestId("change-ci")).toBeNull();
  });
});

/**
 * The page on its own (T-2137): the two states it shows before any decision can be made — it is
 * still reading the change, and the change could not be read. The refusal carries the API's own
 * sentence, because "something went wrong" over a 404 and over a 403 tells a person nothing
 * about whether to ask for access or to go back.
 */
describe("the approval detail page, mounted on its own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const page = (
    <AuthProvider>
      <ApprovalDetailPage project="helsinki" id="chg-1a2b3c4d" />
    </AuthProvider>
  );

  it("announces the wait rather than an empty page", async () => {
    renderPage(page, {
      answer: (url) =>
        url.pathname.includes("/changes/") ? (new Promise<Response>(() => {}) as never) : undefined,
      path: "/projects/helsinki/approvals/chg-1a2b3c4d",
    });
    // The bars say nothing themselves; the wait is announced by the region's own name.
    const waiting = await screen.findByRole("status");
    expect(waiting).toHaveAttribute("aria-busy", "true");
    expect(waiting).toHaveAccessibleName(en.app.loading);
  });

  // T-2834: a change nobody holds answers 404 the second time too, so no Retry; the way back stays.
  it("says in the API's own words that there is no such change, and leads back to the approvals", async () => {
    renderPage(page, {
      answer: (url) =>
        url.pathname.includes("/changes/")
          ? problem(404, "No change 'chg-1a2b3c4d' in helsinki.")
          : undefined,
      path: "/projects/helsinki/approvals/chg-1a2b3c4d",
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("No change 'chg-1a2b3c4d' in helsinki.");
    expect(alert.textContent).not.toContain(en.app.error.generic);
    expect(within(alert).queryByRole("button", { name: en.app.error.retry })).toBeNull();
    expect(screen.getByRole("heading", { level: 1, name: "chg-1a2b3c4d" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.approvals.back })).toBeInTheDocument();
  });

  it("has no axe violation in either", async () => {
    const { container } = renderPage(page, {
      answer: (url) =>
        url.pathname.includes("/changes/") ? problem(403, "You may not read this change.") : undefined,
      path: "/projects/helsinki/approvals/chg-1a2b3c4d",
    });
    await screen.findByRole("alert");
    await expectNoAxeViolations(container);
  });
});
