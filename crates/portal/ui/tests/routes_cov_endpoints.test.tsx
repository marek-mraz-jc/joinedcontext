/**
 * EndpointsPage: the one grid of own, shared and gone-reference rows read by every column, the
 * search by name, the row menu's dialogs and how each is dismissed, an endpoint the assistant or
 * the address opens in the editor, and the editor's refusals.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectOpen } from "./checks";
import { findFormPage, queryFormPage } from "./formPage";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";
import endpointSchema from "./fixtures/kinds/Endpoint.json";
import { sortEachColumn } from "./routes_cov_grid";
import { greenVerdict, isCheck } from "./verdict";

const API = "joinedcontext.com/v1alpha1";
const PATH = "/projects/helsinki/endpoints";

const endpoint = (project: string, name: string, over: Record<string, unknown> = {}, spec: Record<string, unknown> = {}) => ({
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name, namespace: project, labels: { "joinedcontext.com/space": "air" } },
  spec: {
    contextSpaceRef: "air",
    slug: `slug${name.replaceAll("-", "").padEnd(22, "x")}`.slice(0, 26),
    audience: "organization",
    enabledRepresentations: ["ngsi-ld"],
    ...spec,
  },
  status: { phase: "Live" },
  ...over,
});

const PUBLIC_AIR = endpoint("helsinki", "public-air", {}, { audience: "public" });
const NO_MCP = endpoint("helsinki", "quiet-buses", { status: { phase: "Error" } }, { mcp: false, enabledRepresentations: ["ngsi-ld", "geojson"] });
const SHARED = endpoint("espoo", "espoo-bikes");
const GONE = {
  apiVersion: API,
  kind: "SharedSpaceReference",
  metadata: { name: "vantaa-trams", namespace: "helsinki" },
  spec: { endpointRef: { project: "vantaa", name: "trams" }, alias: "trams" },
  status: { phase: "Live" },
};
const SPACE = {
  apiVersion: API,
  kind: "ContextSpace",
  metadata: { name: "air", namespace: "helsinki" },
  spec: {},
  status: { phase: "Live" },
};

const PROJECTS = () => jsonResponse(list([{ name: "helsinki" }, { name: "espoo" }]));

function answering(rows: { own?: unknown[]; shared?: unknown[]; references?: unknown[] } = {}, extra?: (path: string, request: Request) => Response | Promise<Response> | undefined) {
  return async (path: string, request: Request): Promise<Response> => {
    const own = await extra?.(path, request);
    if (own) return own;
    if (path === "/api/v1/projects/helsinki/endpoints") return jsonResponse(list(rows.own ?? [PUBLIC_AIR, NO_MCP]));
    if (path === "/api/v1/projects/espoo/endpoints") return jsonResponse(list(rows.shared ?? [SHARED]));
    if (path === "/api/v1/projects/helsinki/shared") return jsonResponse(list(rows.references ?? [GONE]));
    if (path === "/api/v1/projects/helsinki/spaces") return jsonResponse(list([SPACE]));
    return jsonResponse(list([]));
  };
}

/** What the page proposed or deleted: a write that is neither a draft nor a dry-run check. */
function writes(fetchMock: ReturnType<typeof vi.fn>): string[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method !== "GET")
    .filter((request) => !request.url.includes("/drafts/") && new URL(request.url).searchParams.get("dryRun") !== "All")
    .map((request) => `${request.method} ${new URL(request.url).pathname}`);
}

const table = () => screen.findByRole("table", { name: en.endpoints.title });

describe("the endpoints grid", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sorts by each visible column over own, shared and gone rows", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const grid = await table();
    await within(grid).findByText("espoo-bikes");
    await within(grid).findByText("trams");
    const sorted = await sortEachColumn(user, grid);
    expect(Object.keys(sorted)).toEqual(expect.arrayContaining([en.endpoints.source.label, en.endpoints.field.audience, en.endpoints.calls.label, en.endpoints.field.phase]));
    for (const rows of Object.values(sorted)) expect(rows).toHaveLength(4);
    // Phase reads empty for a row that is not this project's, so those sort after Error and Live.
    expect(sorted[en.endpoints.field.phase].slice(0, 2)).toEqual(["quiet-buses", "public-air"]);
  });

  it("sorts by the columns a person shows from Arrange columns, the space and MCP ones too", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const grid = await table();
    await within(grid).findByText("espoo-bikes");
    await user.click(screen.getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const panel = await screen.findByRole("dialog", { name: en.entityGrid.arrangeColumns });
    for (const label of [en.grid.field.space, en.grid.field.mcp]) {
      await user.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${label}` }));
    }
    await user.keyboard("{Escape}");
    const sorted = await sortEachColumn(user, grid);
    expect(sorted[en.grid.field.space]).toHaveLength(4);
    // MCP reads Off for the one endpoint that turned it off, On for the rest: Off sorts first.
    expect(sorted[en.grid.field.mcp][0]).toBe("quiet-buses");
  });

  it("narrows by the name typed in the header, names what matched nothing and clears from the empty state", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const grid = await table();
    await within(grid).findByText("trams");
    const find = screen.getByRole("searchbox", { name: en.endpoints.find });
    await user.type(find, "buses");
    await waitFor(() => expect(within(grid).queryByText("public-air")).toBeNull());
    expect(within(grid).getByText("quiet-buses")).toBeInTheDocument();
    // A gone reference is found by its own name.
    await user.clear(find);
    await user.type(find, "vantaa-trams");
    await waitFor(() => expect(within(grid).queryByText("quiet-buses")).toBeNull());
    expect(within(grid).getByText("trams")).toBeInTheDocument();
    await user.clear(find);
    await user.type(find, "nothing-like-this");
    expect(await screen.findByText(i18n.t("endpoints.searchNoMatch", { text: "nothing-like-this" }))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.spaces.inside.clearSearch }));
    expect(await screen.findByText("quiet-buses")).toBeInTheDocument();
    expect(find).toHaveValue("");
  });

  it("opens the connector dialog from the header", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    await user.click(await screen.findByRole("button", { name: en.endpoints.hub.button }));
    expect(await screen.findByRole("dialog", { name: en.endpoints.hub.title })).toBeInTheDocument();
  });
});

describe("the row menu of an endpoint", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  async function openMenu(name = "public-air") {
    const user = userEvent.setup();
    const grid = await table();
    const row = (await within(grid).findByText(name)).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    return user;
  }

  it("opens the endpoint's own page", async () => {
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: en.resourceList.open }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/endpoints/public-air"));
  });

  it("copies the public address to the clipboard", async () => {
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = await openMenu();
    // After `userEvent.setup()`, which installs its own clipboard.
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await user.click(await screen.findByRole("menuitem", { name: en.endpoints.copyUrl }));
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(String(writeText.mock.calls[0]).includes(PUBLIC_AIR.spec.slug)).toBe(true);
  });

  it.each([
    [en.grid.yaml.action],
    [en.saveAs.button],
    [en.workspaces.open.action],
    [en.resourceDelete.button],
  ])("opens %s and leaves nothing open and nothing written after Escape", async (action) => {
    const { fetchMock } = await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: action }));
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(writes(fetchMock)).toEqual([]);
  });
});

describe("the endpoint editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens the endpoint the address names, filled from the list, and closes without writing", async () => {
    const { fetchMock } = await renderRoute({ path: `${PATH}?draft=public-air`, projects: PROJECTS, answer: answering() });
    const form = await findFormPage();
    await waitFor(() => expect(within(form).getByDisplayValue("public-air")).toBeInTheDocument());
    expect(within(form).getByText(PUBLIC_AIR.spec.slug)).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(queryFormPage()).toBeNull());
    expect(writes(fetchMock)).toEqual([]);
  });

  it("says why the server refused the proposal and keeps the form", async () => {
    await renderRoute({
      path: `${PATH}?draft=public-air`,
      projects: PROJECTS,
      answer: answering({}, async (path, request) => {
        const url = new URL(request.url);
        if (isCheck(request, url)) {
          return jsonResponse(await greenVerdict(request, { valid: true }));
        }
        if (path === "/api/v1/projects/helsinki/endpoints/public-air" && request.method === "PUT") {
          return problem(403, "Your role may not change the audience of 'public-air'.");
        }
        return undefined;
      }),
    });
    const form = await findFormPage();
    await waitFor(() => expect(within(form).getByDisplayValue("public-air")).toBeInTheDocument());
    await userEvent.click(within(form).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() => expectOpen(within(form).getByRole("button", { name: en.endpoints.propose })));
    await userEvent.click(within(form).getByRole("button", { name: en.endpoints.propose }));
    expect(await within(form).findByText(/Your role may not change the audience/)).toBeInTheDocument();
    expect(queryFormPage()).not.toBeNull();
  });

  it("opens a person who may not propose on the record read only, and Escape closes it", async () => {
    await renderRoute({
      path: `${PATH}?edit=public-air`,
      projects: PROJECTS,
      permissions: { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Endpoint", "ContextSpace", "DataModel", "Policy", "Pipeline", "CkanInstance", "ModelProjection", "SharedSpaceReference"], verbs: ["read"] } }] },
      answer: answering({}, (path) => (path === "/api/v1/projects/helsinki/endpoints/public-air" ? jsonResponse(PUBLIC_AIR) : undefined)),
    });
    const form = await findFormPage();
    expect(await within(form).findByText(en.resourceEdit.readOnlyBadge)).toBeInTheDocument();
    await waitFor(() => expect(within(form).queryByRole("alert")).toBeNull());
    expect(within(form).queryByRole("button", { name: en.resourceEdit.propose })).toBeNull();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(queryFormPage()).toBeNull());
  });
});

const named = (kind: string, name: string, spec: Record<string, unknown> = {}) => ({
  apiVersion: API,
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
});

const LINKML = "id: m\nname: m\nclasses:\n  Car:\n    slots: [id, speed]\nslots:\n  id: {range: string}\n  speed: {range: integer}\n";

/** A published endpoint that names a catalogue, a pipeline and a narrowed dataset of the Car type. */
const PUBLISHED = endpoint("helsinki", "public-air", {}, {
  audience: "public",
  publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "open-data-hub" } } },
  catalog: {
    pipelineRef: { kind: "Pipeline", name: "air-ingest" },
    datasets: [{ name: "cars", title: { en: "Cars" }, description: { en: "Every car" }, types: ["Car"] }],
  },
});

function editorAnswers(model: unknown, fit: unknown) {
  return answering({ own: [PUBLISHED] }, (path) => {
    if (path === "/api/v1/projects/helsinki/datamodels") return jsonResponse(list([model]));
    if (path === "/api/v1/projects/helsinki/ckaninstances") return jsonResponse(list([named("CkanInstance", "open-data-hub")]));
    if (path === "/api/v1/projects/helsinki/pipelines") return jsonResponse(list([named("Pipeline", "air-ingest")]));
    if (path.endsWith("/surface-fit")) return jsonResponse(fit);
    if (path.endsWith("/admission")) return jsonResponse({ admitted: true });
    return undefined;
  });
}

async function openEditorOfRow(name: string) {
  const user = userEvent.setup();
  const grid = await table();
  const row = (await within(grid).findByText(name)).closest("tr") as HTMLElement;
  await user.click(within(row).getByRole("button", { name: /More actions/ }));
  await user.click(await screen.findByRole("menuitem", { name: en.endpoints.edit }));
  return { user, form: await findFormPage() };
}

describe("the editor of a published endpoint", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says what the space's model cannot serve and leads to the model", async () => {
    await renderRoute({
      path: PATH,
      projects: PROJECTS,
      answer: editorAnswers(named("DataModel", "air-model", { contextSpaceRef: "air", linkml: LINKML }), {
        dataModel: "air-model",
        surfaces: [{ representation: "sta", gap: "The model has no numeric property with a unit." }],
      }),
    });
    const { form } = await openEditorOfRow("public-air");
    const gaps = await within(form).findByTestId("surface-gaps");
    expect(within(gaps).getByText("The model has no numeric property with a unit.")).toBeInTheDocument();
    expect(within(form).getByRole("link", { name: i18n.t("endpoints.surfaceFit.openModel", { model: "air-model" }) })).toHaveAttribute(
      "href",
      "/projects/helsinki/models/air-model",
    );
  });

  it("offers to add a model when the space has none, and says so for a gap", async () => {
    await renderRoute({
      path: PATH,
      projects: PROJECTS,
      answer: editorAnswers(named("DataModel", "other", { contextSpaceRef: "elsewhere", linkml: LINKML }), {
        surfaces: [{ representation: "sta", gap: "The space has no model." }],
      }),
    });
    const { form } = await openEditorOfRow("public-air");
    expect(await within(form).findByText("The space has no model.")).toBeInTheDocument();
    expect(within(form).getByRole("link", { name: en.endpoints.surfaceFit.addModel })).toHaveAttribute("href", "/projects/helsinki/models");
  });

  it("proves what ticking a class of the model would serve", async () => {
    await renderRoute({
      path: PATH,
      projects: PROJECTS,
      answer: editorAnswers(named("DataModel", "air-model", { contextSpaceRef: "air", linkml: LINKML }), { surfaces: [] }),
    });
    const { user, form } = await openEditorOfRow("public-air");
    const projection = (await within(form).findByText(en.endpoints.section.projection)).closest("section") as HTMLElement;
    await user.click(await within(projection).findByLabelText("Car"));
    await waitFor(() => expect(within(form).getByTestId("projection-summary")).toHaveTextContent(/1/));
    const [proof] = await within(form).findAllByText(en.endpoints.proof.title);
    await user.click(proof);
    expect(proof.closest("details")).toHaveAttribute("open");
  });

  it("opens on a model whose source cannot be read without offering any class", async () => {
    await renderRoute({
      path: PATH,
      projects: PROJECTS,
      answer: editorAnswers(named("DataModel", "air-model", { contextSpaceRef: "air", linkml: "classes: [unterminated: {" }), { surfaces: [] }),
    });
    const { form } = await openEditorOfRow("public-air");
    await waitFor(() => expect(within(form).getAllByDisplayValue("public-air").length).toBeGreaterThan(0));
    const projection = (await within(form).findByText(en.endpoints.section.projection)).closest("section") as HTMLElement;
    expect(within(projection).queryByLabelText("Car")).toBeNull();
  });
});

describe("the endpoints grid beside the usage and the details panel", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  const usage = {
    name: "helsinki",
    from: "2026-10-01",
    to: "2026-10-07",
    days: [],
    totals: {},
    endpoints: [{ name: "public-air", total: 12, totals: { "ngsi-ld": 9, mcp: 3 }, callers: { person: 5, agent: 7 } }],
  };

  it("counts the calls of an own endpoint and shows the split in its details with the kind's editable fields", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: PATH,
      projects: PROJECTS,
      answer: answering({}, (path) => {
        if (path === "/api/v1/kinds/Endpoint/schema") return jsonResponse(endpointSchema);
        if (path.endsWith("/usage/endpoints")) return jsonResponse(usage);
        return undefined;
      }),
    });
    const grid = await table();
    const row = (await within(grid).findByText("public-air")).closest("tr") as HTMLElement;
    await waitFor(() => expect(within(row).getByText("12")).toBeInTheDocument());
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.grid.details }));
    const dialog = await screen.findByRole("dialog");
    const calls = await within(dialog).findByRole("list", { name: i18n.t("endpoints.calls.bySurface") });
    expect(within(calls).getByText("9")).toBeInTheDocument();
    // The representations of the kind's schema are a set of choices, each named as the Portal names it.
    expect(await within(dialog).findByRole("group", { name: en.endpoints.field.representations })).toBeInTheDocument();
  });

  it("copes with a refused clipboard when the address is copied", async () => {
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.reject(new Error("denied")));
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const grid = await table();
    const row = (await within(grid).findByText("public-air")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.endpoints.copyUrl }));
    expect(writeText).toHaveBeenCalledTimes(1);
    // The row menu closes as usual and the page stays as it was.
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(screen.getByText("public-air")).toBeInTheDocument();
  });
});

describe("the routed forms of the endpoints page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens a blank form on /endpoints/new once the spaces have answered, with the first space chosen", async () => {
    let release: () => void = () => undefined;
    const spaces = new Promise<void>((resolve) => {
      release = resolve;
    });
    await renderRoute({
      path: `${PATH}/new`,
      projects: PROJECTS,
      answer: answering({}, async (path) => {
        if (path === "/api/v1/projects/helsinki/spaces") {
          await spaces;
          return jsonResponse(list([SPACE]));
        }
        return undefined;
      }),
    });
    // Nothing opens while the spaces are on their way.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(queryFormPage()).toBeNull();
    release();
    const form = await findFormPage();
    expect(within(form).getByRole("heading", { name: en.endpoints.add })).toBeInTheDocument();
    await waitFor(() => expect(within(form).getByText("air")).toBeInTheDocument());
  });

  it("opens the endpoint ?edit= names once, even when the list is read again", async () => {
    let reads = 0;
    const { client } = await renderRoute({
      path: `${PATH}?edit=public-air`,
      projects: PROJECTS,
      answer: answering({}, (path) => {
        if (path === "/api/v1/projects/helsinki/endpoints") {
          reads += 1;
          return jsonResponse(list([endpoint("helsinki", "public-air", { status: { phase: reads > 1 ? "Draft" : "Live" } }, { audience: "public" })]));
        }
        return undefined;
      }),
    });
    const form = await findFormPage();
    expect(within(form).getByRole("heading", { name: en.endpoints.edit })).toBeInTheDocument();
    await client.invalidateQueries({ queryKey: ["projects", "helsinki", "endpoints"] });
    await client.refetchQueries();
    await waitFor(() => expect(reads).toBeGreaterThan(1));
    expect(within(await findFormPage()).getByRole("heading", { name: en.endpoints.edit })).toBeInTheDocument();
  });
});
