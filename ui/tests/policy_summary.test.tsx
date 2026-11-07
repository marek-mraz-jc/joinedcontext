/**
 * T-5591: one Policy opens on what it allows, in words, with who wrote it down and its last
 * change; the fields fold under "Show the manifest fields". A reader reads all of it without a
 * click, and the heading is the Policy's title with a "Read only" badge, not "View …".
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { PolicySummary } from "../src/pages/policies/PolicySummary";
import { afterEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";
import { policySchema } from "../src/schemas/kinds";

const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };

const policy = (name: string, title: string, spec: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { contextSpaceRef: { kind: "ContextSpace", name: "helsinki" }, ...spec },
  status: { phase: "Live" },
});

const STEWARD_STATUS = policy("bikes-ops-steward", "The steward sets a station's status", {
  assigner: "did:web:hel.fi",
  assignee: { kind: "role", id: "steward" },
  operations: ["updateOps"],
  information: [{ entities: [{ type: "BikeHireDockingStation" }], propertyNames: ["status"] }],
  q: "status!=\"retired\"",
  validity: { from: "2026-10-01T00:00:00Z" },
});

const NO_WRITES = policy("no-public-writes", "Nobody outside writes", {
  effect: "prohibition",
  assignee: { kind: "group", id: "public" },
  operations: ["updateOps"],
  information: [{ entities: [{ type: "BikeHireDockingStation" }] }],
});

const merged = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "42", namespace: "helsinki" },
  author: { name: "Anna Steward" },
  createdAt: "2026-10-20T08:00:00Z",
  decision: { at: "2026-10-20T09:30:00Z", by: "Pekka Approver" },
  status: { phase: "Merged", lane: "standard", plan: {} },
  summary: { kind: "update", params: { kind: "Policy", name: "bikes-ops-steward" } },
};

afterEach(() => vi.restoreAllMocks());

function answering(stored: ReturnType<typeof policy>, history: () => Response) {
  return (path: string, request: Request) => {
    if (request.method !== "GET") return undefined;
    if (path === "/api/v1/projects/helsinki/policies") return jsonResponse(list([stored]));
    if (path === `/api/v1/projects/helsinki/policies/${stored.metadata.name}`) return jsonResponse(stored);
    if (path === "/api/v1/projects/helsinki/changes/history") return history();
    return undefined;
  };
}

describe("a Policy reads as what it allows (T-5591)", () => {
  it("a grant names who, the operations, the type with its attribute and the q filter, without a click", async () => {
    await renderRoute({
      path: "/projects/helsinki/policies/bikes-ops-steward/edit?lang=en",
      answer: answering(STEWARD_STATUS, () => jsonResponse({ ...list([merged]), next: null })),
      permissions: READER,
    });
    const summary = await screen.findByTestId("policy-summary");
    const sentence = within(summary).getByTestId("policy-summary-sentence").textContent ?? "";
    expect(sentence).toContain("steward");
    expect(sentence).toContain("BikeHireDockingStation");
    expect(sentence).toContain("status");
    expect(sentence).toContain('where status ≠ "retired"');
    expect(sentence).toMatch(/may change/);
    expect(within(summary).getByText("did:web:hel.fi")).toBeVisible();
    expect(within(summary).getByText("No end date")).toBeInTheDocument();
    expect(
      await within(summary).findByText(/proposed by Anna Steward, approved by Pekka Approver/),
    ).toBeInTheDocument();
    // The title alone, closed for a reader with the reason beside it.
    expect(screen.getAllByText("The steward sets a station's status").length).toBeGreaterThan(0);
    expect(screen.queryByText(/^View /)).toBeNull();
    expect(screen.getByText("Read only")).toBeInTheDocument();
    // The fields are there, folded, for a reader.
    const fields = screen.getByText("Show the manifest fields").closest("details");
    expect(fields).not.toBeNull();
    expect(fields).not.toHaveAttribute("open");
  });

  it("a prohibition reads as what may not happen; a history the person may not read is not recorded", async () => {
    await renderRoute({
      path: "/projects/helsinki/policies/no-public-writes/edit?lang=en",
      answer: answering(NO_WRITES, () => problem(403, "You may not read this project's changes.")),
      permissions: READER,
    });
    const summary = await screen.findByTestId("policy-summary");
    expect(within(summary).getByTestId("policy-summary-sentence").textContent).toMatch(/may not/);
    // No assigner stored and no history readable: both say so.
    await waitFor(() => expect(within(summary).getAllByText("Not recorded")).toHaveLength(2));
  });

  it("a person who may change it gets the fields open under the summary", async () => {
    await renderRoute({
      path: "/projects/helsinki/policies/bikes-ops-steward/edit?lang=en",
      answer: answering(STEWARD_STATUS, () => jsonResponse({ ...list([]), next: null })),
    });
    expect(await screen.findByTestId("policy-summary")).toBeInTheDocument();
    expect(screen.getByText("Show the manifest fields").closest("details")).toHaveAttribute("open");
    expect(await screen.findByRole("button", { name: "Propose change" })).toBeInTheDocument();
  });
});

describe("a Policy with rules[] (F-UI3-015)", () => {
  it("says each rule and sends the person to the YAML view for them", async () => {
    const ruled = policy("parking-grants", "Who reads and who writes parking", {
      assigner: "did:web:hel.fi",
      rules: [
        { assignee: { kind: "role", id: "helsinki-readers" }, operations: ["retrieveOps"] },
        { assignee: { kind: "group", id: "stewards" }, operations: ["updateOps"] },
      ],
    });
    await renderRoute({
      path: "/projects/helsinki/policies/parking-grants/edit?lang=en",
      answer: answering(ruled, () => jsonResponse({ ...list([]), next: null })),
    });
    const summary = await screen.findByTestId("policy-summary");
    const sentence = within(summary).getByTestId("policy-summary-sentence").textContent ?? "";
    expect(sentence).toContain("helsinki-readers");
    expect(sentence).toContain("stewards");
    expect(within(summary).getByTestId("policy-summary-rules")).toHaveTextContent(
      "This Policy grants through 2 rules, one sentence each above.",
    );
  });
});

describe("editing a Policy with rules[] (F-R2UI4-003)", () => {
  it("offers no grantee or operation field beside the rules, which a save would drop", async () => {
    const ruled = policy("parking-grants", "Who reads and who writes parking", {
      assigner: "did:web:hel.fi",
      rules: [{ assignee: { kind: "role", id: "helsinki-readers" }, operations: ["retrieveOps"] }],
    });
    await renderRoute({
      path: "/projects/helsinki/policies/parking-grants/edit?lang=en",
      answer: answering(ruled, () => jsonResponse({ ...list([]), next: null })),
    });
    await screen.findByTestId("policy-summary-rules");
    const form = (await screen.findByRole("button", { name: /Propose change/ })).closest("form") as HTMLElement;
    expect(within(form).queryByText(i18n.t("policies.field.assignee"))).toBeNull();
    expect(within(form).queryByText(i18n.t("policies.field.operations"))).toBeNull();
    const schema = policySchema(i18n.t, [], [], [], true) as { required: string[]; properties: Record<string, unknown> };
    expect(schema.required).toEqual(["name", "contextSpaceRef"]);
    for (const member of ["assignee", "operations", "information", "q", "effect"]) expect(schema.properties).not.toHaveProperty(member);
    expect(policySchema(i18n.t).properties).toHaveProperty("assignee");
  });
});

describe("the summary alone", () => {
  it("dates a bounded validity and reads an empty Policy as a request for choices", async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 403 })));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PolicySummary
            project="helsinki"
            policy={{
              name: "draft",
              contextSpaceRef: "",
              assignee: { kind: "role", id: "" },
              operations: [],
              validity: { from: "2026-10-01T00:00:00Z", to: "2026-12-31T00:00:00Z" },
            }}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(screen.getByTestId("policy-summary-sentence").textContent).toBe(i18n.t("policies.sentence.empty"));
    expect(screen.getByText("Oct 1, 2026")).toBeInTheDocument();
    expect(screen.getByText("Dec 31, 2026")).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByText("Not recorded")).toHaveLength(2));
    vi.unstubAllGlobals();
  });
});
