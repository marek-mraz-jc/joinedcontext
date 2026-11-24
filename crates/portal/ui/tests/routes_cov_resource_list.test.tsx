/**
 * The generic kind list's remaining branches: each kind's own edit form read and written back
 * through the row menu, a refused proposal, the search on titles, and the kinds with pages of
 * their own that load lazily.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { hasEditForm, KindList, ResourceListPage } from "../src/routes/ResourceListPage";
import { digestOf } from "../src/api/digest";
import { toAgentProfileManifest } from "../src/schemas/agentprofile";
import type { AgentProfileForm } from "../src/schemas/agentprofile";
import { toBlueprintManifest } from "../src/schemas/blueprint";
import type { BlueprintForm } from "../src/schemas/blueprint";
import { toDataAgreementManifest } from "../src/schemas/dataagreement";
import type { DataAgreementForm } from "../src/schemas/dataagreement";
import { toDataModelManifest } from "../src/schemas/datamodel";
import type { DataModelForm } from "../src/schemas/datamodel";
import { toDataOfferManifest } from "../src/schemas/dataoffer";
import type { DataOfferForm } from "../src/schemas/dataoffer";
import { toDataSpaceParticipantManifest } from "../src/schemas/dataspaceparticipant";
import type { DataSpaceParticipantForm } from "../src/schemas/dataspaceparticipant";
import { toEnvironmentManifest } from "../src/schemas/environment";
import type { EnvironmentForm } from "../src/schemas/environment";
import { toMappingManifest } from "../src/schemas/mapping";
import type { MappingForm } from "../src/schemas/mapping";
import { toModelProjectionManifest } from "../src/schemas/modelprojection";
import type { ModelProjectionForm } from "../src/schemas/modelprojection";
import { toScopeDefinitionManifest } from "../src/schemas/scopedefinition";
import type { ScopeDefinitionForm } from "../src/schemas/scopedefinition";
import { json, list, problem, renderPage } from "./page_contract";

// Monaco draws on a canvas and starts a worker, neither of which jsdom has; a textarea with the
// same contract stands in, so the YAML view's own work is what runs here.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));
const PAGES: string[] = ["endpoints", "dashboards", "pipelines", "policies", "subscriptions", "csrs", "datasources", "apps", "syncsources", "spaces"];
vi.mock("../src/routes/EndpointsPage", () => ({ EndpointsPage: ({ project }: { project: string }) => <p>the endpoints page of {project}</p> }));
vi.mock("../src/routes/DashboardsPage", () => ({ DashboardsPage: ({ project }: { project: string }) => <p>the dashboards page of {project}</p> }));
vi.mock("../src/routes/PipelinesPage", () => ({ PipelinesPage: ({ project }: { project: string }) => <p>the pipelines page of {project}</p> }));
vi.mock("../src/routes/PoliciesPage", () => ({ PoliciesPage: ({ project }: { project: string }) => <p>the policies page of {project}</p> }));
vi.mock("../src/routes/SubscriptionsPage", () => ({ SubscriptionsPage: ({ project }: { project: string }) => <p>the subscriptions page of {project}</p> }));
vi.mock("../src/routes/RegistrationsPage", () => ({ RegistrationsPage: ({ project }: { project: string }) => <p>the csrs page of {project}</p> }));
vi.mock("../src/pages/datasources/DataSourcesPage", () => ({ DataSourcesPage: ({ project }: { project: string }) => <p>the datasources page of {project}</p> }));
vi.mock("../src/pages/apps/AppsCatalog", () => ({ AppsCatalog: ({ project }: { project: string }) => <p>the apps page of {project}</p> }));
vi.mock("../src/pages/sync/SyncSourcesPage", () => ({ SyncSourcesPage: ({ project }: { project: string }) => <p>the syncsources page of {project}</p> }));
vi.mock("../src/routes/SpacesPage", () => ({ SpacesPage: ({ project }: { project: string }) => <p>the spaces page of {project}</p> }));
vi.mock("../src/pages/mqttbrokers/MqttBrokersPage", () => ({
  MqttBrokersPage: ({ project, edit }: { project: string; edit?: string }) => <p>the brokers page of {project} {edit ?? "none"}</p>,
}));
vi.mock("../src/pages/scripts/ScriptsPage", () => ({
  ScriptsPage: ({ project }: { project: string }) => <p>the scripts page of {project}</p>,
}));

/** A resource each kind's own form accepts as it is, written by the same code the form writes with. */
const VALID: [plural: string, kind: string, stored: () => unknown][] = [
  ["mappings", "Mapping", () => toMappingManifest(undefined, { name: "sample", contextSpaceRef: "air", source: { name: "a", version: "1" }, target: { name: "b", version: "1" }, tests: [{ input: "in.json", expect: "out.json" }] } as unknown as MappingForm)],
  ["datamodels", "DataModel", () => toDataModelManifest(undefined, { name: "sample", lifecycle: "draft" } as unknown as DataModelForm)],
  ["dataagreements", "DataAgreement", () => toDataAgreementManifest("org", { name: "sample", role: "provider", remoteParticipant: "did:web:partner.example.org", agreementId: "agr-1", state: "requested" } as unknown as DataAgreementForm)],
  ["dataoffers", "DataOffer", () => toDataOfferManifest("org", { name: "sample", contextSpaceRef: "air", endpointRefs: ["ep"], policy: "open" } as unknown as DataOfferForm)],
  ["projections", "ModelProjection", () => toModelProjectionManifest("org", { name: "sample", contextSpaceRef: "air", dataModelRef: { name: "m", version: "1" }, classes: [{ name: "Thing" }] } as unknown as ModelProjectionForm)],
  ["scopedefinitions", "ScopeDefinition", () => toScopeDefinitionManifest("org", { name: "sample", scopeString: "/geo/sk" } as unknown as ScopeDefinitionForm)],
  ["blueprints", "Blueprint", () => toBlueprintManifest({ name: "sample", version: "1.0.0", riskClass: "yellow", allowedRoles: ["steward"], parameterSchema: '{"type":"object"}', templates: [{ name: "source", template: "kind: DataSource" }] } as unknown as BlueprintForm)],
  ["dataspaceparticipants", "DataSpaceParticipant", () => toDataSpaceParticipantManifest({ name: "sample", did: "did:web:hel.example.org", credentialIssuer: "https://issuer.example.org", connectorUrl: "https://connector.example.org", engine: "rainbow" } as unknown as DataSpaceParticipantForm)],
  ["environments", "Environment", () => toEnvironmentManifest({ name: "sample" } as unknown as EnvironmentForm)],
  ["agentprofiles", "AgentProfile", () => toAgentProfileManifest({
    name: "sample", role: "steward", runtime: { image: "ghcr.io/example/agent", digest: `sha256:${"a".repeat(64)}` },
    model: { provider: "anthropic", name: "model-a", maxTokensPerRun: 1000 },
    limits: { stepsPerRun: 10, wallClock: "PT10M", concurrentRunsPerOrganization: 1, requestsPerMinute: 10, maxResponseBytes: 1000 },
    workspace: { cpu: "1", memory: "1Gi", ephemeralStorage: "1Gi" },
  } as unknown as AgentProfileForm)],
];

const KINDS: [plural: string, kind: string][] = [
  ["mqttbrokers", "MqttBroker"],
  ["mappings", "Mapping"],
  ["datamodels", "DataModel"],
  ["dataagreements", "DataAgreement"],
  ["dataoffers", "DataOffer"],
  ["projections", "ModelProjection"],
  ["scopedefinitions", "ScopeDefinition"],
  ["blueprints", "Blueprint"],
  ["dataspaceparticipants", "DataSpaceParticipant"],
  ["environments", "Environment"],
  ["agentprofiles", "AgentProfile"],
  ["knowledgesources", "KnowledgeSource"],
  ["assistantdeployments", "AssistantDeployment"],
];

const manifest = (kind: string, name: string, extra: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "org", ...(extra.metadata as object) },
  spec: {},
  status: { phase: "Live" },
});

const SUBSCRIBE = "sensors/${clientid}/commands";
const ALL = { project: "org", bootstrap: true, grants: [] };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function kindList(plural: string, stored: unknown, sent: { method: string; path: string; body: Record<string, unknown> }[] = []) {
  return renderPage(<KindList project="org" plural={plural} />, {
    path: `/organization/${plural}`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ALL);
      if (request.method !== "GET") {
        sent.push({ method: request.method, path: url.pathname, body: (await request.json()) as Record<string, unknown> });
        return problem(422, "Refused by the test server.");
      }
      if (url.pathname.endsWith(`/${plural}/sample`)) return json(stored);
      if (url.pathname.endsWith(`/${plural}`)) return json(list([stored]));
      return undefined;
    },
  });
}

async function openEdit(): Promise<HTMLElement> {
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /More actions for/ }));
  await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
  const dialog = await screen.findByRole("dialog");
  await waitFor(() => expect(within(dialog).queryByText(/Reading sample/)).toBeNull());
  return dialog;
}

describe("each kind's own edit form", () => {
  it.each(KINDS)("%s: Edit opens the kind's form on the stored resource", async (plural, kind) => {
    kindList(plural, manifest(kind, "sample"));
    const dialog = await openEdit();
    // The form read the stored resource: its name is in the form, read only.
    await waitFor(() => expect(within(dialog).getAllByDisplayValue("sample").length).toBeGreaterThan(0));
    expect(within(dialog).getAllByDisplayValue("sample")[0]).toHaveAttribute("readonly");
  });

  it.each(VALID)("%s: Propose in the form writes the stored resource back as a change to it", async (plural, kind, stored) => {
    const sent: { method: string; path: string; body: Record<string, unknown> }[] = [];
    kindList(plural, stored(), sent);
    const dialog = await openEdit();
    await waitFor(() => expect(within(dialog).getAllByDisplayValue("sample").length).toBeGreaterThan(0));
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent.length).toBeGreaterThan(0));
    expect(sent[0].method).toBe("PUT");
    expect(sent[0].path).toBe(`/api/v1/projects/org/${plural}/sample`);
    expect(sent[0].body.kind).toBe(kind);
    expect((sent[0].body.metadata as { name: string }).name).toBe("sample");
  });

  it("an MQTT broker's edit keeps what the form has no field for and puts the roles back as written", async () => {
    const sent: { method: string; path: string; body: Record<string, unknown> }[] = [];
    const stored = {
      ...manifest("MqttBroker", "sample"),
      spec: { address: "mqtt.example.org", roles: [{ name: "readers", serviceAccounts: ["sensor"], subscribe: [SUBSCRIBE] }] },
    };
    kindList("mqttbrokers", stored, sent);
    const dialog = await openEdit();
    expect(await within(dialog).findByDisplayValue("readers")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent.length).toBeGreaterThan(0));
    expect(sent[0].method).toBe("PUT");
    const body = sent[0].body as { kind: string; metadata: { name: string }; spec: { address: string; roles: unknown[] } };
    expect(body.kind).toBe("MqttBroker");
    expect(body.metadata.name).toBe("sample");
    expect(body.spec.address).toBe("mqtt.example.org");
    expect(body.spec.roles).toEqual([{ name: "readers", serviceAccounts: ["sensor"], subscribe: [SUBSCRIBE] }]);
    expect(await within(dialog).findByText("Refused by the test server.")).toBeInTheDocument();
  });

  it("a knowledge source's edit proposes the source with its own spec and no upload limits", async () => {
    const sent: { method: string; path: string; body: Record<string, unknown> }[] = [];
    const stored = {
      ...manifest("KnowledgeSource", "sample"),
      spec: { source: "website", visibility: "internal", startUrls: ["https://example.org/"] },
    };
    kindList("knowledgesources", stored, sent);
    const dialog = await openEdit();
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent.length).toBeGreaterThan(0));
    const body = sent[0].body as { kind: string; spec: Record<string, unknown> };
    expect(body.kind).toBe("KnowledgeSource");
    expect(body.spec.startUrls).toEqual(["https://example.org/"]);
    expect(body.spec).not.toHaveProperty("upload");
  });
});

describe("the kinds with a page of their own", () => {
  it("opens the MQTT brokers page for its plural and hands it the project and the resource to edit", async () => {
    renderPage(<ResourceListPage project="helsinki" plural="mqttbrokers" edit="sensors" />, { answer: () => undefined });
    expect(await screen.findByText("the brokers page of helsinki sensors")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it.each(PAGES)("%s: opens its own page and no table", async (plural) => {
    renderPage(<ResourceListPage project="helsinki" plural={plural} />, { answer: () => undefined });
    expect(await screen.findByText(`the ${plural} page of helsinki`)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("opens the scripts page for its plural", async () => {
    renderPage(<ResourceListPage project="helsinki" plural="scripts" />, { answer: () => undefined });
    expect(await screen.findByText("the scripts page of helsinki")).toBeInTheDocument();
  });

  it("knows which plurals have an edit page: its own page, a form, or a create form, and no other", () => {
    expect(hasEditForm("spaces")).toBe(true);
    expect(hasEditForm("mappings")).toBe(true);
    expect(hasEditForm("dataoffers")).toBe(true);
    expect(hasEditForm("roles")).toBe(false);
    expect(hasEditForm("toString")).toBe(false);
  });
});

describe("the generic list, remaining states", () => {
  it("sorts the rows by name as the person reads it, a title before the name", async () => {
    const rows = [
      { ...manifest("Role", "zulu"), metadata: { name: "zulu", namespace: "org", title: { en: "Alpha team" } } },
      { ...manifest("Role", "alpha"), metadata: { name: "alpha", namespace: "org", title: { en: "Zeta team" } } },
      manifest("Role", "mike"),
    ];
    renderPage(<KindList project="org" plural="roles" />, {
      path: "/organization/roles",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(ALL) : url.pathname.endsWith("/roles") ? json(list(rows)) : undefined),
    });
    const table = await screen.findByRole("table");
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(4));
    const order = () =>
      within(table)
        .getAllByRole("row")
        .slice(1)
        .map((row) => within(row).getAllByRole("link")[0].textContent);
    expect(order()).toEqual(["Alpha team", "Zeta team", "mike"]);
    const sort = within(table).getByRole("button", { name: en.resourceList.name });
    await userEvent.click(sort);
    expect(order()).toEqual(["Alpha team", "mike", "Zeta team"]);
    await userEvent.click(sort);
    expect(order()).toEqual(["Zeta team", "mike", "Alpha team"]);
  });
});

describe("creating from the list", () => {
  function creating(answerPost: () => Response) {
    const sent: { method: string; path: string; dryRun: boolean; body: Record<string, unknown> }[] = [];
    renderPage(<KindList project="org" plural="knowledgesources" />, {
      path: "/organization/knowledgesources",
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ALL);
        if (request.method === "POST") {
          const dryRun = url.searchParams.get("dryRun") === "All";
          const body = (await request.json()) as Record<string, unknown>;
          sent.push({ method: request.method, path: url.pathname, dryRun, body });
          const judged = Object.fromEntries(Object.entries(body).filter(([key]) => key !== "draft"));
          return dryRun
            ? json({ valid: true, verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(judged) } })
            : answerPost();
        }
        if (url.pathname.endsWith("/knowledgesources")) return json(list([]));
        return undefined;
      },
    });
    return sent;
  }

  async function fillAndPropose(): Promise<HTMLElement> {
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: en.kinds.KnowledgeSource.new }))[0]);
    const dialog = await screen.findByRole("dialog");
    const name = await waitFor(() => {
      const found = dialog.querySelector<HTMLElement>("#root_name");
      if (!found) throw new Error("form not ready");
      return found;
    });
    await user.type(name, "city-site");
    const start = dialog.querySelector<HTMLElement>("#root_startUrls_0");
    if (start) await user.type(start, "https://example.org/");
    await user.click(within(dialog).getByRole("button", { name: en.form.check }));
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(propose);
    return dialog;
  }

  it("says in the form why the proposal was refused and keeps what was typed", async () => {
    const sent = creating(() => problem(422, "A source needs something to read."));
    const dialog = await fillAndPropose();
    expect(await within(dialog).findByText("A source needs something to read.")).toBeInTheDocument();
    expect(dialog.querySelector<HTMLInputElement>("#root_name")?.value).toBe("city-site");
    const proposed = sent.find((one) => !one.dryRun);
    expect(proposed?.path).toBe("/api/v1/projects/org/knowledgesources");
    expect((proposed?.body.metadata as { name: string }).name).toBe("city-site");
  });

  it("closes the form with Cancel and sends nothing", async () => {
    const sent = creating(() => problem(500, "unused"));
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: en.kinds.KnowledgeSource.new }))[0]);
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("button", { name: en.form.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sent).toEqual([]);
  });
});

describe("the generic list, more states", () => {
  it("says a kind the API does not serve is an address with no page, with no retry", async () => {
    renderPage(<KindList project="org" plural="roles" />, {
      path: "/organization/roles",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(ALL) : url.pathname.endsWith("/roles") ? problem(404, "no such kind here") : undefined),
    });
    expect(await screen.findByRole("heading", { name: en.app.notFound.title })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("adds a Context Space column when a resource names its space, and sorts by it", async () => {
    const rows = [
      { ...manifest("Role", "one"), metadata: { name: "one", namespace: "org", labels: { "joinedcontext.com/space": "zebra" } } },
      { ...manifest("Role", "two"), metadata: { name: "two", namespace: "org", labels: { "joinedcontext.com/space": "air" } } },
    ];
    renderPage(<KindList project="org" plural="roles" />, {
      path: "/organization/roles",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(ALL) : url.pathname.endsWith("/roles") ? json(list(rows)) : undefined),
    });
    const table = await screen.findByRole("table");
    const sort = await within(table).findByRole("button", { name: en.grid.field.space });
    const names = () =>
      within(table)
        .getAllByRole("row")
        .slice(1)
        .map((row) => within(row).getAllByRole("link")[0].textContent);
    expect(names()).toEqual(["one", "two"]);
    await userEvent.click(sort);
    expect(names()).toEqual(["two", "one"]);
    await userEvent.click(sort);
    expect(names()).toEqual(["one", "two"]);
  });

  it("shows no space column when no resource names one", async () => {
    renderPage(<KindList project="org" plural="roles" />, {
      path: "/organization/roles",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(ALL) : url.pathname.endsWith("/roles") ? json(list([manifest("Role", "one")])) : undefined),
    });
    const table = await screen.findByRole("table");
    await within(table).findByText("one");
    expect(within(table).queryByRole("button", { name: en.grid.field.space })).toBeNull();
  });
});

describe("creating from the list, the other ways", () => {
  it("reads the YAML typed in the dialog back into the form's fields", async () => {
    renderPage(<KindList project="org" plural="knowledgesources" />, {
      path: "/organization/knowledgesources",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(ALL) : undefined),
    });
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: en.kinds.KnowledgeSource.new }))[0]);
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("tab", { name: en.form.view.yaml }));
    const editor = await within(dialog).findByRole("textbox", { name: "YAML" });
    await user.clear(editor);
    await user.click(editor);
    await user.paste(
      "apiVersion: joinedcontext.com/v1alpha1\nkind: KnowledgeSource\nmetadata:\n  name: typed-site\n  namespace: org\nspec:\n  source: website\n  visibility: internal\n",
    );
    await user.click(within(dialog).getByRole("tab", { name: en.form.view.form }));
    await waitFor(() => expect(dialog.querySelector<HTMLInputElement>("#root_name")?.value).toBe("typed-site"));
  });

  it("shows the change a created resource opened, and closes the form", async () => {
    renderPage(<KindList project="org" plural="knowledgesources" />, {
      path: "/organization/knowledgesources",
      answer: async (url, request) => {
        if (url.pathname.endsWith("/permissions/me")) return json(ALL);
        if (request.method === "POST") {
          const body = (await request.json()) as Record<string, unknown>;
          if (url.searchParams.get("dryRun") === "All") {
            const judged = Object.fromEntries(Object.entries(body).filter(([key]) => key !== "draft"));
            return json({ valid: true, verdict: { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(judged) } });
          }
          return json(
            { apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-00000077", namespace: "org" }, status: { lane: "red", phase: "PendingApproval" } },
            202,
          );
        }
        return undefined;
      },
    });
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: en.kinds.KnowledgeSource.new }))[0]);
    const dialog = await screen.findByRole("dialog");
    const name = await waitFor(() => {
      const found = dialog.querySelector<HTMLElement>("#root_name");
      if (!found) throw new Error("form not ready");
      return found;
    });
    await user.type(name, "city-site");
    const start = dialog.querySelector<HTMLElement>("#root_startUrls_0");
    if (start) await user.type(start, "https://example.org/");
    await user.click(within(dialog).getByRole("button", { name: en.form.check }));
    const propose = within(dialog).getByRole("button", { name: en.resourceList.propose });
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(propose);
    expect(await screen.findByText(/chg-00000077/)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
