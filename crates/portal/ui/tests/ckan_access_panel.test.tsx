/**
 * A CKAN instance's Access panel (T-3046, PF-107, UI-44): who manages it, groups first, each with
 * the binding it comes from; giving it to a group proposes the instance's own Role and RoleBinding
 * as one Change, removing a subject edits that binding; every control stays in place, disabled with
 * the reason, for someone who may not approve a binding.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/ckan/CkanAccessPanel.tsx.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CkanAccessPanel } from "../src/pages/ckan/CkanAccessPanel";
import { expectNoViolations } from "./checks";
import { list, renderPage } from "./page_contract";

const V = "joinedcontext.com/v1alpha1";
const CHANGE = {
  apiVersion: V,
  kind: "Change",
  metadata: { name: "chg-0000003a", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 2, update: 0, delete: 0 } },
};

const role = (name: string, rules: unknown[]) => ({ apiVersion: V, kind: "Role", metadata: { name, namespace: "org" }, spec: { rules } });
const binding = (name: string, roleName: string, subjects: unknown[], scope: unknown) => ({
  apiVersion: V,
  kind: "RoleBinding",
  metadata: { name, namespace: "org" },
  spec: { subjects, role: roleName, scope },
});

const ORG_ADMIN = role("org-admin", [{ kinds: ["CkanInstance", "RoleBinding", "Role"], verbs: ["propose", "approve", "delete"] }]);
const OWN = role("ckan-admin-helsinki-hel-fi", [
  { kinds: ["CkanInstance"], verbs: ["propose", "approve", "delete"], constraints: [{ field: "metadata.name", in: ["hel-fi"] }] },
]);
const ADMINS = binding("admins", "org-admin", [{ user: "demo.steward@hel.fi" }], { organization: "hel" });
const ENDPOINT = {
  apiVersion: V,
  kind: "Endpoint",
  metadata: { name: "helsinki-events", namespace: "helsinki" },
  spec: { audience: "public", publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } } } },
};

interface Sent {
  method: string;
  path: string;
  dryRun: boolean;
  body: unknown;
}

function renderPanel({ instance = "hel-fi", bindings = [ADMINS], roles = [ORG_ADMIN], approve = true, orgReader = true } = {}) {
  const read: string[] = [];
  const sent: Sent[] = [];
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
    });
  renderPage(<CkanAccessPanel project="helsinki" instance={instance} known={["hel-fi", "open-data-2"]} />, {
    path: "/projects/helsinki/ckan",
    answer: async (url, request) => {
      const path = url.pathname;
      if (path.endsWith("/permissions/me")) {
        return reply(200, {
          project: "org",
          bootstrap: false,
          grants: orgReader
            ? [{ rule: { kinds: ["Role", "RoleBinding", "Group"], verbs: approve ? ["read", "propose", "approve"] : ["read"] } }]
            : [],
        });
      }
      if (request.method !== "GET") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        sent.push({ method: request.method, path, dryRun, body: await request.json() });
        return reply(dryRun ? 200 : 202, dryRun ? { valid: true, verdict: { ok: true } } : CHANGE);
      }
      if (path.startsWith("/api/v1/projects/org/")) read.push(path);
      if (!orgReader && path.startsWith("/api/v1/projects/org/")) {
        return reply(404, { title: "Not Found", status: 404, detail: "plural roles not found in project org" });
      }
      if (path === "/api/v1/projects/org/roles") return reply(200, list(roles));
      if (path === "/api/v1/projects/org/rolebindings") return reply(200, list(bindings));
      if (path === "/api/v1/projects/org/groups") {
        return reply(200, list([{ apiVersion: V, kind: "Group", metadata: { name: "ckan-editors", namespace: "org" }, spec: { members: [] } }]));
      }
      if (path === "/api/v1/projects/helsinki/endpoints") return reply(200, list([ENDPOINT]));
      return undefined;
    },
  });
  return { proposals: () => sent.filter((s) => !s.dryRun), read };
}

const panel = async (instance = "hel-fi") =>
  (await screen.findByRole("heading", { name: `Access to ${instance}` })).closest("section") as HTMLElement;

/** The add button once the caller's permissions are read: until then it is refused while loading. */
async function addButton(section: HTMLElement, enabled = true): Promise<HTMLElement> {
  const add = await within(section).findByRole("button", { name: en.ckan.access.addShort });
  await waitFor(() => expect(section).not.toHaveTextContent(en.app.loading));
  if (enabled) expect(add).not.toHaveAttribute("aria-disabled");
  return add;
}

describe("a CKAN instance's Access panel", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("lists the administrator and the endpoints that publish to the instance", async () => {
    renderPanel();
    const section = await panel();
    expect(await within(section).findByText("demo.steward@hel.fi")).toBeInTheDocument();
    expect(within(section).getByText("through the role org-admin")).toBeInTheDocument();
    expect(await within(section).findByRole("link", { name: "helsinki-events" })).toBeInTheDocument();
    // The administrator's right is not this panel's to take away.
    expect(within(section).queryByRole("button", { name: /^Remove demo\.steward/ })).toBeNull();
    await expectNoViolations(section);
  });

  it("gives the instance to a group: the role and the binding as one Change", async () => {
    const { proposals } = renderPanel();
    const section = await panel();
    const which = await within(section).findByLabelText(en.ckan.access.whichGroup);
    // The groups are read once the caller's permissions say they may read them.
    await within(which).findByRole("option", { name: "ckan-editors" });
    await userEvent.selectOptions(which, "ckan-editors");
    await userEvent.click(await addButton(section));
    expect(await within(section).findByText("chg-0000003a")).toBeInTheDocument();
    expect(proposals()).toHaveLength(1);
    const [sent] = proposals();
    expect(sent.path).toBe("/api/v1/projects/helsinki/import");
    const manifests = (sent.body as { manifests: { kind: string; metadata: { name: string }; spec: unknown }[] }).manifests;
    expect(manifests.map((m) => `${m.kind}/${m.metadata.name}`)).toEqual([
      "Role/ckan-admin-helsinki-hel-fi",
      "RoleBinding/ckan-admin-helsinki-hel-fi",
    ]);
    expect(manifests[1].spec).toEqual({
      subjects: [{ group: "ckan-editors" }],
      role: "ckan-admin-helsinki-hel-fi",
      scope: { project: "helsinki" },
    });
  });

  it("refuses an empty choice and a typed e-mail that names nobody in words, and proposes nothing", async () => {
    const { proposals } = renderPanel();
    const section = await panel();
    await userEvent.click(await addButton(section));
    expect(await within(section).findByRole("alert")).toHaveTextContent(en.ckan.access.refused.noSubject);
    await userEvent.selectOptions(within(section).getByLabelText(en.ckan.access.kind), "user");
    // An address is no subject (ADR-N-061): only a person picked, or a pasted id, is.
    await userEvent.type(within(section).getByRole("combobox", { name: en.ckan.access.email }), "jana@hel.fi{Enter}");
    await userEvent.click(within(section).getByRole("button", { name: en.ckan.access.addShort }));
    expect(await within(section).findByRole("alert")).toHaveTextContent(en.ckan.access.refused.noSubject);
    expect(proposals()).toEqual([]);
  });

  it("removes one subject by editing the instance's binding, and the last one through the delete dialog", async () => {
    const own = binding("ckan-admin-helsinki-hel-fi", "ckan-admin-helsinki-hel-fi", [{ group: "ckan-editors" }, { user: "zoe@hel.fi" }], { project: "helsinki" });
    const { proposals } = renderPanel({ roles: [ORG_ADMIN, OWN], bindings: [ADMINS, own] });
    const section = await panel();
    const holders = await within(section).findByRole("list", { name: "Who manages hel-fi" });
    // Groups first, then people.
    expect(within(holders).getAllByRole("listitem").map((item) => item.firstElementChild?.textContent)).toEqual([
      "Group ckan-editors",
      "demo.steward@hel.fi",
      "zoe@hel.fi",
    ]);
    await addButton(section);
    await userEvent.click(within(section).getByRole("button", { name: "Remove ckan-editors from hel-fi" }));
    expect(await within(section).findByText("chg-0000003a")).toBeInTheDocument();
    const [sent] = proposals();
    expect(sent).toMatchObject({ method: "PUT", path: "/api/v1/projects/org/rolebindings/ckan-admin-helsinki-hel-fi" });
    expect((sent.body as { spec: { subjects: unknown[] } }).spec.subjects).toEqual([{ user: "zoe@hel.fi" }]);
  });

  it("keeps every control in place, disabled with the reason, for someone who may not approve a binding", async () => {
    const { proposals } = renderPanel({ approve: false });
    const section = await panel();
    const add = await addButton(section, false);
    expect(add).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(add);
    expect(section).toHaveTextContent(en.ckan.access.denied);
    expect(proposals()).toEqual([]);
  });

  it("tells someone who may not read the organization's roles who sees the list, and asks nothing", async () => {
    // An editor, an approver and a viewer met "The list could not be loaded: plural roles not
    // found in project org" on the CKAN page (T-3131).
    const { read } = renderPanel({ orgReader: false });
    const section = await panel();
    expect(await within(section).findByTestId("ckan-access-hidden")).toHaveTextContent(en.ckan.access.hidden);
    expect(within(section).queryByRole("alert")).toBeNull();
    expect(read.filter((path) => !path.endsWith("/permissions/me"))).toEqual([]);
  });

  it("shows the other instance of the project as the administrator's alone", async () => {
    renderPanel({ instance: "open-data-2", roles: [ORG_ADMIN, OWN] });
    const section = await panel("open-data-2");
    const holders = await within(section).findByRole("list", { name: "Who manages open-data-2" });
    expect(within(holders).getAllByRole("listitem")).toHaveLength(1);
    expect(within(section).getByText(en.ckan.access.noneAttached)).toBeInTheDocument();
  });
});
