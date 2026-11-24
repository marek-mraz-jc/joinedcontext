/**
 * T-7588: Access pages, the cases their own tests leave out: a new group proposed from the YAML
 * view where no route holds the change, a group's members edited and proposed, a role edited and
 * proposed, the organization's role shown in a project, a service account's grant across the
 * organization, and its access read only once a person opens it.
 */
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { Groups, NewGroupDialog } from "../src/pages/access/Groups";
import { Roles } from "../src/pages/access/Roles";
import { grantLabel, ServiceAccounts } from "../src/pages/access/ServiceAccounts";
import { answeringChecks, checksSoFar } from "./checks";
import { json, list, renderPage } from "./page_contract";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

const V = "joinedcontext.com/v1alpha1";
const change = (name: string, namespace = "org") => ({ apiVersion: V, kind: "Change", metadata: { name, namespace }, status: { lane: "red", phase: "PendingApproval" } });
const GROUP = { apiVersion: V, kind: "Group", metadata: { name: "stewards", namespace: "org" }, spec: { members: [{ user: "b7c1e0f4-1111-4222-8333-944455556666" }] } };

function rights(kinds: string[]) {
  return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds, verbs: ["read", "propose", "approve", "delete"] } }] });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a new group where no route holds the change", () => {
  it("shows the change it opened in the dialog and closes on Close", async () => {
    const user = userEvent.setup();
    const posted: unknown[] = [];
    const closed = vi.fn();
    renderPage(<NewGroupDialog open onOpenChange={closed} />, {
      path: "/organization/groups",
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return rights(["Group"]);
        if (url.pathname.startsWith("/api/v1/projects/org/drafts/") && request.method === "PUT") {
          const draft = (await request.clone().json()) as { manifest?: unknown };
          return json({ project: "org", kind: "Group", name: "traffic", manifest: draft.manifest ?? draft, touchedBy: "jana.kovacova", touchedKind: "person", version: 1, updatedAt: "2026-11-24T08:00:00Z" });
        }
        if (url.pathname === "/api/v1/projects/org/groups" && request.method === "POST") {
          posted.push(await request.clone().json());
          return json(change("chg-00000051"), 202);
        }
        return undefined;
      },
    });
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("tab", { name: en.form.view.yaml }));
    fireEvent.change(await within(dialog).findByRole("textbox", { name: "YAML" }), {
      target: { value: ["apiVersion: joinedcontext.com/v1alpha1", "kind: Group", "metadata:", "  name: traffic", "  namespace: org", "spec:", "  members:", "    - user: 2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e"].join("\n") },
    });
    // Back on the form, the fields hold what the YAML says.
    await user.click(within(dialog).getByRole("tab", { name: en.form.view.form }));
    expect(within(dialog).getByLabelText(new RegExp(en.access.groups.field.name))).toHaveValue("traffic");
    await user.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() => expect(checksSoFar()).toContain("POST /api/v1/projects/org/groups"));
    await user.click(within(dialog).getByRole("button", { name: en.access.groups.propose }));
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]).toMatchObject({ kind: "Group", metadata: { name: "traffic" }, spec: { members: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }] } });
    const notice = await screen.findByRole("dialog");
    expect(await within(notice).findByText(/chg-00000051/)).toBeInTheDocument();
    await user.click(within(notice).getAllByRole("button", { name: en.resourceDelete.close }).at(-1) as HTMLElement);
    expect(closed).toHaveBeenCalledWith(false);
  });
});

describe("a group's members edited from the list", () => {
  it("opens the members form on the stored group and proposes the edit", async () => {
    const user = userEvent.setup();
    const sent: { method: string; body: unknown }[] = [];
    renderPage(<Groups project="org" />, {
      path: "/organization/groups",
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return rights(["Group", "RoleBinding"]);
        if (url.pathname === "/api/v1/projects/org/groups/stewards" && request.method !== "GET") {
          sent.push({ method: request.method, body: await request.clone().json() });
          return json(change("chg-00000052"), 202);
        }
        if (url.pathname === "/api/v1/projects/org/groups/stewards") return json(GROUP);
        if (url.pathname === "/api/v1/projects/org/groups") return json(list([GROUP]));
        return undefined;
      },
    });
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    const row = (await screen.findByText("stewards")).closest("tr") as HTMLElement;
    // The guarded button is replaced once the rights are read: query it again until it is open.
    await waitFor(() => expect(within(row).getByRole("button", { name: "Edit stewards" })).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(within(row).getByRole("button", { name: "Edit stewards" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit stewards" });
    expect(await within(dialog).findByDisplayValue(/b7c1e0f4/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].body).toMatchObject({ kind: "Group", metadata: { name: "stewards" }, spec: { members: [{ user: "b7c1e0f4-1111-4222-8333-944455556666" }] } });
  });
});

describe("a service account's grants and access", () => {
  const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, options);

  it("names a grant across the organization, in one place, and a bare role", () => {
    expect(grantLabel(t, "viewer", { organization: "hel" })).toBe(i18n.t("access.accounts.grantOrg", { role: "viewer" }));
    expect(grantLabel(t, "space-writer", { contextSpace: "parking" })).toBe("space-writer in parking");
    expect(grantLabel(t, "viewer", undefined)).toBe("viewer");
  });

  it("reads what an account may do only once a person opens it", async () => {
    const asked: string[] = [];
    renderPage(<ServiceAccounts project="helsinki" />, {
      path: "/projects/helsinki/settings/accounts",
      answer: (url) => {
        asked.push(`${url.pathname}${url.search}`);
        if (url.pathname.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["ServiceAccount"], verbs: ["read"] } }] });
        if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts") {
          return json(
            list([
              {
                apiVersion: V,
                kind: "ServiceAccount",
                metadata: { name: "vendor-push", namespace: "helsinki" },
                spec: { owner: { user: "jana.kovacova" }, purpose: "VendorX pushes", roles: [{ role: "viewer", scope: { organization: "hel" } }], credentials: [{ kind: "oauth-client", name: "main" }] },
              },
            ]),
          );
        }
        return undefined;
      },
    });
    expect(await screen.findByText(i18n.t("access.accounts.grantOrg", { role: "viewer" }))).toBeInTheDocument();
    const before = asked.length;
    expect(asked.some((path) => path.includes("vendor-push/"))).toBe(false);
    // jsdom does not open a <details> from its summary: the disclosure is opened as a browser does it.
    const details = screen.getByText(en.access.agent.open).closest("details") as HTMLDetailsElement;
    details.open = true;
    fireEvent(details, new Event("toggle"));
    await waitFor(() => expect(asked.slice(before).some((path) => /serviceaccount_access|vendor-push/.test(path))).toBe(true));
  });
});

/** Opens a row's Edit once the guard let it, proposes the stored manifest and returns what was sent. */
async function editAndPropose(user: ReturnType<typeof userEvent.setup>, name: string, sent: { method: string; path: string; body: unknown }[]) {
  await waitFor(() => expect(screen.getByRole("button", { name: `Edit ${name}` })).not.toHaveAttribute("aria-disabled", "true"));
  await user.click(screen.getByRole("button", { name: `Edit ${name}` }));
  const dialog = await screen.findByRole("dialog", { name: `Edit ${name}` });
  await user.click(await within(dialog).findByRole("button", { name: en.resourceEdit.propose }));
  await waitFor(() => expect(sent).toHaveLength(1));
  return sent[0];
}

describe("a role edited where the organization's role shows in a project", () => {
  it("lists the organization's role beside the project's and proposes an edit of it at its home", async () => {
    const user = userEvent.setup();
    const sent: { method: string; path: string; body: unknown }[] = [];
    const role = (name: string, namespace: string) => ({ apiVersion: V, kind: "Role", metadata: { name, namespace }, spec: { rules: [{ kinds: ["Pipeline"], verbs: ["read"] }] } });
    renderPage(<Roles project="helsinki" />, {
      path: "/projects/helsinki/settings/roles",
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return json({ project: url.pathname.split("/")[4], bootstrap: false, grants: [{ rule: { kinds: ["Role", "Pipeline"], verbs: ["read", "propose", "approve", "delete"] } }] });
        if (request.method !== "GET" && url.pathname.includes("/roles/")) {
          sent.push({ method: request.method, path: url.pathname, body: await request.clone().json() });
          return json(change("chg-00000053"), 202);
        }
        if (url.pathname === "/api/v1/projects/org/roles/auditor") return json(role("auditor", "org"));
        if (url.pathname === "/api/v1/projects/helsinki/roles") return json(list([role("steward", "helsinki")]));
        if (url.pathname === "/api/v1/projects/org/roles") return json(list([role("auditor", "org")]));
        return undefined;
      },
    });
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    expect(await screen.findByText("auditor")).toBeInTheDocument();
    expect(screen.getByText("steward")).toBeInTheDocument();
    const proposed = await editAndPropose(user, "auditor", sent);
    expect(proposed.path).toBe("/api/v1/projects/org/roles/auditor");
    expect(proposed.body).toMatchObject({ kind: "Role", metadata: { name: "auditor", namespace: "org" }, spec: { rules: [{ kinds: ["Pipeline"], verbs: ["read"] }] } });
  });
});

describe("a service account edited", () => {
  it("proposes the stored account back through its form", async () => {
    const user = userEvent.setup();
    const sent: { method: string; path: string; body: unknown }[] = [];
    const account = {
      apiVersion: V,
      kind: "ServiceAccount",
      metadata: { name: "vendor-push", namespace: "helsinki" },
      spec: { owner: { user: "b7c1e0f4-1111-4222-8333-944455556666" }, purpose: "VendorX pushes", roles: [{ role: "viewer", scope: { project: "helsinki" } }], credentials: [{ kind: "oauth-client", name: "main" }] },
    };
    renderPage(<ServiceAccounts project="helsinki" />, {
      path: "/projects/helsinki/settings/accounts",
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["ServiceAccount", "Role"], verbs: ["read", "propose"] } }] });
        if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts/vendor-push" && request.method !== "GET") {
          sent.push({ method: request.method, path: url.pathname, body: await request.clone().json() });
          return json(change("chg-00000054", "helsinki"), 202);
        }
        if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts/vendor-push") return json(account);
        if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts") return json(list([account]));
        if (url.pathname === "/api/v1/projects/org/roles") return json(list([{ apiVersion: V, kind: "Role", metadata: { name: "viewer", namespace: "org" }, spec: { rules: [] } }]));
        return undefined;
      },
    });
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    const proposed = await editAndPropose(user, "vendor-push", sent);
    expect(proposed.body).toMatchObject({ kind: "ServiceAccount", metadata: { name: "vendor-push", namespace: "helsinki" }, spec: { purpose: "VendorX pushes", roles: [{ role: "viewer", scope: { project: "helsinki" } }] } });
  });
});
