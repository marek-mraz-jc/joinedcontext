// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/endpoints/ShareFlow.tsx.
/**
 * T-3275: sharing an Endpoint with another project, guided: the share is the project in
 * `allowedProjects` and one Policy whose validity is the share's end; revoking ends that Policy
 * now as it takes the project off, so no other endpoint of the space keeps reading for it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { asSharedSees, knownGroupNames, revokeBundle, shareBundle, ShareFlow, shareState } from "../src/pages/endpoints/ShareFlow";
import { InRouter } from "./pageHarness";

const endpoint = (spec: object): Manifest =>
  ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "air", namespace: "helsinki" },
    spec: { contextSpaceRef: "ovzdusie", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project-list", allowedProjects: ["praha"], ...spec },
    status: { phase: "Live" },
  }) as Manifest;
const policy = (project: string, to?: string): Manifest =>
  ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: `air-${project}`, namespace: "helsinki" },
    spec: { assignee: { kind: "group", id: project }, operations: ["retrieveOps"], ...(to ? { validity: { to } } : {}) },
    status: { phase: "Live" },
  }) as Manifest;

describe("what a share is made of (T-3275)", () => {
  it("lists the project on the Endpoint and grants its group the chosen types until the end", () => {
    const { endpoint: updated, policy: grant } = shareBundle(endpoint({}), "ovzdusie", "bbsk", ["AirQualityObserved"], "2026-12-31T22:59:59.000Z");
    expect(updated.spec).toMatchObject({ audience: "project-list", allowedProjects: ["praha", "bbsk"] });
    expect("status" in updated).toBe(false);
    expect(grant.metadata.name).toBe("air-bbsk");
    expect(grant.spec).toEqual({
      contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
      assigner: "did:web:{orgDomain}",
      assignee: { kind: "group", id: "bbsk" },
      operations: ["retrieveOps"],
      information: [{ entities: [{ type: "AirQualityObserved" }] }],
      validity: { to: "2026-12-31T22:59:59.000Z" },
    });
    expect(shareBundle(endpoint({}), "s", "praha", ["A"], undefined).endpoint.spec).toMatchObject({ allowedProjects: ["praha"] });
    expect(shareBundle(endpoint({}), "s", "x", ["A"], undefined).policy.spec).not.toHaveProperty("validity");
  });

  it("revokes by taking the project off and ending its grant now", () => {
    const [updated, ended] = revokeBundle(endpoint({}), policy("praha"), "praha", "2026-10-08T10:00:00.000Z");
    expect(updated.spec).toMatchObject({ allowedProjects: [] });
    expect(ended.spec).toMatchObject({ validity: { to: "2026-10-08T10:00:00.000Z" } });
    expect(revokeBundle(endpoint({}), undefined, "praha", "x")).toHaveLength(1);
  });

  it("tells an open share from one with an end and one that ended, and shows only what the projection keeps", () => {
    const now = new Date("2026-10-08T00:00:00Z");
    expect(shareState(policy("p"), now).state).toBe("open");
    expect(shareState(policy("p", "2026-12-31T00:00:00Z"), now).state).toBe("until");
    expect(shareState(policy("p", "2026-01-01T00:00:00Z"), now).state).toBe("ended");
    expect(shareState(undefined, now).state).toBe("noGrant");
    expect(asSharedSees({ id: "u", type: "T", pm10: 1, note: "x", secret: "s" }, ["pm10", "secret"], ["secret"])).toEqual({ id: "u", type: "T", pm10: 1 });
    expect(asSharedSees({ id: "u", type: "T", a: 1 }, [], [])).toEqual({ id: "u", type: "T", a: 1 });
  });
});

describe("which Group names a share knows of (F-R2UI3-005, T-3852)", () => {
  const group = (name: string) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Group", metadata: { name, namespace: "x" }, spec: {} });
  const list = (...names: string[]) => ({ hidden: false, data: { items: names.map(group) } });
  it("joins the organization's and the project's groups once both are read", () => {
    expect(knownGroupNames(list("org-a"), list("praha"))).toEqual(new Set(["org-a", "praha"]));
  });
  it("counts an organization list the person does not read as empty", () => {
    expect(knownGroupNames({ hidden: true }, list("praha"))).toEqual(new Set(["praha"]));
  });
  it("knows nothing while a list loads or when the project's list is hidden", () => {
    expect(knownGroupNames({ hidden: false }, list("praha"))).toBeUndefined();
    expect(knownGroupNames(list("org-a"), { hidden: false })).toBeUndefined();
    expect(knownGroupNames(list("org-a"), { hidden: true })).toBeUndefined();
  });
});

describe("the share flow on an Endpoint's page (T-3275)", () => {
  let imports: { query: string; manifests: Manifest[]; conflictPolicy?: string }[];
  let asked: string[];
  function show(spec: object, policies: Manifest[], permissions?: object, orgDenied = false) {
    imports = [];
    asked = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        const url = new URL(request.url);
        const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
        asked.push(url.pathname);
        if (orgDenied && url.pathname === "/api/v1/projects/org/permissions/me") {
          return json({ type: "about:blank", title: "Not Found", status: 404, detail: "project 'org' not found" }, 404);
        }
        if (permissions && url.pathname.endsWith("/permissions/me")) return json(permissions);
        if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }, { name: "praha" }, { name: "bbsk" }] });
        if (url.pathname.endsWith("/import") && request.method === "POST") {
          const sent = (await request.json()) as { manifests: Manifest[]; conflictPolicy?: string };
          imports.push({ query: url.search, manifests: sent.manifests, conflictPolicy: sent.conflictPolicy });
          return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-00000042", namespace: "helsinki" }, status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 1, delete: 0 } } }, 202);
        }
        if (url.pathname.includes("/ngsi-ld/v1/entities")) {
          return json([{ id: "urn:ngsi-ld:AirQualityObserved:1", type: "AirQualityObserved", pm10: 12, stationNote: "maintenance at 9" }]);
        }
        return json({ items: [] });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ShareFlow project="helsinki" endpoint={endpoint(spec)} space="ovzdusie" types={["AirQualityObserved"]} slots={["pm10", "stationNote"]} policies={policies} />
          </InRouter>
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

  it("says where each share stands and stops one with its grant in one change", async () => {
    show({ allowedProjects: ["praha"] }, [policy("praha", "2027-01-01T00:00:00Z")]);
    const shared = (await screen.findByText("praha")).closest("li") as HTMLElement;
    expect(shared).toHaveTextContent("until Jan 1, 2027");
    await userEvent.click(within(shared).getByRole("button", { name: "Stop sharing with praha" }));
    await waitFor(() => expect(imports.map((i) => i.query)).toEqual(["?dryRun=All", ""]));
    // The Endpoint and the Policy exist: the bundle replaces them, as an update does.
    expect(imports[1].conflictPolicy).toBe("replace");
    const [ended, updated] = imports[1].manifests;
    expect(updated.spec).toMatchObject({ allowedProjects: [] });
    expect((ended.spec as { validity: { to: string } }).validity.to).toMatch(/^\d{4}-\d\d-\d\dT/);
    expect(await screen.findByText("chg-00000042")).toBeInTheDocument();
  });

  it("previews what the other project sees without hidden attributes, and proposes the share with its end", async () => {
    show({ allowedProjects: [], projection: { hiddenAttributes: ["stationNote"] } }, []);
    const pick = await screen.findByLabelText(en.endpoints.shareFlow.project);
    await within(pick).findByRole("option", { name: "bbsk" });
    await userEvent.selectOptions(pick, "bbsk");
    const preview = await screen.findByTestId("share-preview");
    // T-5516: the preview reads as rows, never as JSON text.
    expect(within(preview).getByText("pm10")).toBeInTheDocument();
    expect(within(preview).getByText("12")).toBeInTheDocument();
    expect(preview.textContent).not.toContain("{\n");
    expect(preview).not.toHaveTextContent("maintenance");
    await userEvent.type(screen.getByLabelText(en.endpoints.shareFlow.until), "2026-12-31");
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.shareFlow.propose }));
    await waitFor(() => expect(imports).toHaveLength(2));
    // No group bbsk exists yet: it is proposed empty with the share, and the page says so.
    expect(screen.getByText(/does not exist yet/)).toBeInTheDocument();
    const [group, grant, shared] = imports[1].manifests;
    expect(group).toMatchObject({ kind: "Group", metadata: { name: "bbsk", namespace: "helsinki" }, spec: { members: [] } });
    expect(shared.spec).toMatchObject({ allowedProjects: ["bbsk"] });
    expect(grant.metadata.name).toBe("air-bbsk");
    expect((grant.spec as { validity: { to: string } }).validity.to).toMatch(/^2026-12-31T|^2027-01-01T/);
  });

  it("proposes no Group over one the person cannot see (F-R2UI3-005)", async () => {
    show({ allowedProjects: [] }, [], { grants: [{ rule: { kinds: ["Endpoint", "Policy"], verbs: ["read", "propose"] } }] });
    const pick = await screen.findByLabelText(en.endpoints.shareFlow.project);
    await within(pick).findByRole("option", { name: "bbsk" });
    await userEvent.selectOptions(pick, "bbsk");
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.shareFlow.propose }));
    await waitFor(() => expect(imports).toHaveLength(2));
    expect(imports[1].manifests.map((manifest) => manifest.kind)).not.toContain("Group");
    expect(screen.queryByText(/does not exist yet/)).toBeNull();
  });

  it("asks for no group list the person does not read (T-3852)", async () => {
    show({ allowedProjects: [] }, [], { grants: [{ rule: { kinds: ["Endpoint"], verbs: ["read"] } }] });
    const pick = await screen.findByLabelText(en.endpoints.shareFlow.project);
    await within(pick).findByRole("option", { name: "bbsk" });
    await waitFor(() => expect(asked.filter((path) => path.endsWith("/permissions/me"))).toHaveLength(2));
    expect(asked.filter((path) => path.endsWith("/groups"))).toEqual([]);
  });

  it("asks for no organization group list when no binding covers the organization (T-5754)", async () => {
    show({ allowedProjects: [] }, [], { grants: [{ rule: { kinds: ["Group", "Endpoint"], verbs: ["read"] } }] }, true);
    const pick = await screen.findByLabelText(en.endpoints.shareFlow.project);
    await within(pick).findByRole("option", { name: "bbsk" });
    await waitFor(() => expect(asked).toContain("/api/v1/projects/helsinki/groups"));
    expect(asked).toContain("/api/v1/projects/org/permissions/me");
    expect(asked).not.toContain("/api/v1/projects/org/groups");
  });

  it("offers no share for an endpoint every project or anyone reads already", async () => {
    show({ audience: "organization" }, []);
    expect(await screen.findByText(en.endpoints.shareFlow.already.organization)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.endpoints.shareFlow.propose })).toBeNull();
  });
});
