/**
 * PoliciesPage: the grid sorted by every column, a policy proposed from a draft (accepted and
 * refused), a policy edited through its row menu, and the manifest the form writes.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { digestOf } from "../src/api/digest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { PoliciesPage, toPolicyEnvelope } from "../src/routes/PoliciesPage";
import type { PolicyForm } from "../src/routes/PoliciesPage";
import { json, list, problem, renderPage } from "./page_contract";
import { sortEachColumn } from "./routes_cov_grid";

const PROJECT = "helsinki";
const DOMAIN = "hel.fi";

const FORM: PolicyForm = {
  name: "air-readers",
  contextSpaceRef: "air",
  assignee: { kind: "role", id: "public" },
  operations: ["retrieveOps"],
};

const READERS = {
  ...(toPolicyEnvelope(PROJECT, DOMAIN, FORM) as object),
  status: { phase: "Live" },
};
const WRITERS = {
  ...(toPolicyEnvelope(PROJECT, DOMAIN, {
    ...FORM,
    name: "bus-writers",
    contextSpaceRef: "buses",
    assignee: { kind: "group", id: "stewards" },
    operations: ["updateOps"],
  }) as object),
  status: { phase: "Error" },
};
const SPACES = ["air", "buses"].map((name) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name, namespace: PROJECT },
  spec: {},
}));

const propose = () =>
  json({ grants: [{ rule: { kinds: ["Policy"], verbs: ["propose"] } }] });

describe("the policies page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sorts by each column and reads the text of every one", async () => {
    const user = userEvent.setup();
    renderPage(<PoliciesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/policies`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/policies")) return json(list([WRITERS, READERS]));
        if (url.pathname.endsWith("/spaces")) return json(list(SPACES));
        return undefined;
      },
    });
    const table = await screen.findByRole("table", { name: en.policies.title });
    const sorted = await sortEachColumn(user, table);
    for (const column of [
      en.policies.field.name,
      en.policies.field.space,
      en.policies.field.assignee,
      en.policies.field.grants,
      en.resourceList.phase,
    ]) {
      expect(sorted[column], column).toHaveLength(2);
    }
    // Space: air before buses; grantee: public before stewards; phase: Error before Live.
    expect(sorted[en.policies.field.space][0]).toContain("air-readers");
    expect(sorted[en.policies.field.assignee][0]).toContain("air-readers");
    expect(sorted[en.policies.field.grants][0]).toContain("air-readers");
    expect(sorted[en.resourceList.phase][0]).toContain("bus-writers");
  });

  function draftPage(after: (dryRun: boolean) => Response) {
    const sent: { dryRun: boolean }[] = [];
    renderPage(<PoliciesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/policies?draft=air-readers`,
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.includes("/drafts/Policy/air-readers")) {
          return json({ version: 1, manifest: toPolicyEnvelope(PROJECT, DOMAIN, FORM), verdict: null });
        }
        if (request.method === "POST" && url.pathname.endsWith("/policies")) {
          const dryRun = url.searchParams.get("dryRun") === "All";
          const body = (await request.json()) as Record<string, unknown>;
          sent.push({ dryRun });
          if (sent.length === 1) {
            const judged = Object.fromEntries(Object.entries(body).filter(([key]) => key !== "draft"));
            return json({
              valid: true,
              verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(judged) },
            });
          }
          return after(dryRun);
        }
        if (url.pathname.endsWith("/spaces")) return json(list(SPACES));
        if (url.pathname.endsWith("/policies")) return json(list([]));
        return undefined;
      },
    });
    return sent;
  }

  async function checked() {
    const user = userEvent.setup();
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("air-readers")).toBeInTheDocument());
    const propose = within(dialog).getByRole("button", { name: en.policies.propose });
    if (propose.getAttribute("aria-disabled") === "true") {
      await user.click(within(dialog).getByRole("button", { name: en.form.check }));
      await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    }
    await user.click(propose);
    return dialog;
  }

  it("proposes a policy from a draft, closes the form and names the Change it opened", async () => {
    const sent = draftPage((dryRun) =>
      dryRun
        ? json({ valid: true })
        : json(
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ChangeProposal",
              metadata: { name: "chg-7e2c4b10", namespace: PROJECT },
              summary: { key: "change.summary.create", params: { kind: "Policy", name: "air-readers", fields: 0 } },
              author: { name: "Jana Kováčová", email: "jana@hel.fi" },
              createdAt: "2026-10-01T09:00:00Z",
              status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
            },
            202,
          ),
    );
    await checked();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sent.at(-1)?.dryRun).toBe(false);
    expect(await screen.findByRole("link", { name: en.changes.review })).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/approvals/chg-7e2c4b10`,
    );
  });

  it("keeps the dialog open and says why when the server refuses the policy", async () => {
    draftPage(() => problem(403, "You may not propose a Policy on space 'air'."));
    const dialog = await checked();
    expect(await within(dialog).findByText(/You may not propose a Policy on space 'air'/)).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("edits a policy through the row menu, with the stored manifest in the form", async () => {
    const user = userEvent.setup();
    renderPage(<PoliciesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/policies`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return propose();
        if (url.pathname.endsWith("/policies/air-readers")) return json(READERS);
        if (url.pathname.endsWith("/policies")) return json(list([READERS]));
        if (url.pathname.endsWith("/spaces")) return json(list(SPACES));
        return undefined;
      },
    });
    await screen.findByRole("table", { name: en.policies.title });
    const row = (await screen.findAllByText("air-readers"))[0].closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /air-readers/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("air-readers")).toBeInTheDocument());
    expect(within(dialog).getByText(en.policies.summary.title)).toBeInTheDocument();
  });
});

describe("the manifest a policy form writes", () => {
  it("leaves out what is blank or unset and keeps what is a number or a flag", () => {
    const written = toPolicyEnvelope(PROJECT, DOMAIN, {
      ...FORM,
      q: undefined,
      scopeQ: "  ",
      geoQ: "near;maxDistance==500",
      temporalQ: "",
      ...({ throttling: 60, dryRun: false, note: null } as object),
    } as PolicyForm) as { spec: Record<string, unknown> };
    expect(written.spec).toEqual({
      contextSpaceRef: { kind: "ContextSpace", name: "air" },
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
      geoQ: "near;maxDistance==500",
      throttling: 60,
      dryRun: false,
      assigner: `did:web:${DOMAIN}`,
    });
  });
});
