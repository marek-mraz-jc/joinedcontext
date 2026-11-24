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
import { json, list, problem, renderPage } from "./page_contract";

vi.mock("../src/pages/mqttbrokers/MqttBrokersPage", () => ({
  MqttBrokersPage: ({ project, edit }: { project: string; edit?: string }) => <p>the brokers page of {project} {edit ?? "none"}</p>,
}));
vi.mock("../src/pages/scripts/ScriptsPage", () => ({
  ScriptsPage: ({ project }: { project: string }) => <p>the scripts page of {project}</p>,
}));

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
