/**
 * T-2765 (DM-61, DM-62, UI-84): the Data models page opens on the project's models, a row opens
 * the model's own page, and that page shows the model six ways without editing it.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { jsonResponse, list, renderRoute } from "./pageHarness";
import { narrowsAndOrders } from "./grid_checks";
import { usesOfModel } from "../src/pages/models/modelUsage";
import { yamlParts } from "../src/pages/models/ModelViews";
import { lastChangeOf, TYPES_SHOWN } from "../src/pages/models/ModelsList";
import { organizationImportName } from "../src/pages/models/OrganizationModels";
import type { Manifest } from "../src/api/manifest";

const manifest = (kind: string, name: string, spec: Record<string, unknown>, extra: Record<string, unknown> = {}) =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind, metadata: { name, namespace: "helsinki" }, spec, ...extra }) as Manifest;

const AIR = `# the city's air
id: https://hel.fi/models/air
name: air
classes:
  AirQualityObserved:
    slots: [pm10, level, refStation]
  Station:
    slots: [name]
slots:
  pm10: { range: float, required: true }
  level: { range: AirLevel }
  refStation: { range: Station }
  name: { range: string }
enums:
  AirLevel:
    permissible_values:
      good: { title: { en: Good } }
      bad: {}
`;

const MODELS = [
  manifest(
    "DataModel",
    "air",
    { contextSpaceRef: "ilma", linkml: AIR, version: "1.2.0", lifecycle: "published", classes: ["AirQualityObserved", "Station"] },
    { metadata: { name: "air", title: "Air quality" }, status: { conditions: [{ lastTransitionTime: "2026-09-20T08:00:00Z" }] } },
  ),
  manifest("DataModel", "bikes", { contextSpaceRef: "mobility", linkml: "./bikes.linkml.yaml", version: "1.0.0", classes: ["BikeHireDockingStation"] }),
  manifest("DataModel", "events", { contextSpaceRef: "culture", linkml: "./events.linkml.yaml", version: "0.1.0", lifecycle: "draft", classes: [] }),
];

const PROJECT_LISTS: Record<string, Manifest[]> = {
  datamodels: MODELS,
  spaces: [manifest("ContextSpace", "ilma", {}), manifest("ContextSpace", "mobility", {}), manifest("ContextSpace", "culture", {})],
  endpoints: [
    manifest("Endpoint", "air-public", { contextSpaceRef: "ilma", slug: "a" }),
    manifest("Endpoint", "air-internal", { contextSpaceRef: "ilma", slug: "b" }),
    manifest("Endpoint", "bikes-public", { contextSpaceRef: "mobility", slug: "c" }),
  ],
  pipelines: [manifest("Pipeline", "air-ingest", { output: { type: "AirQualityObserved", mode: "upsert" } })],
  apps: [manifest("App", "air-map", { dataNeeds: [{ contextSpaceRef: "ilma", types: ["AirQualityObserved"] }] })],
  dashboards: [manifest("Dashboard", "city", { title: "City", pages: [{ layers: ["air-layer"] }] })],
  layers: [manifest("Layer", "air-layer", { sourceEndpointRef: "air-public", entityType: "AirQualityObserved" })],
  mappings: [manifest("Mapping", "air-to-sdm", { source: { name: "air", version: "1" }, target: { name: "sdm-air", version: "1" } })],
  subscriptions: [manifest("Subscription", "air-alerts", { contextSpaceRef: "ilma" })],
};

function answer(path: string): Response | undefined {
  const plural = /^\/api\/v1\/projects\/helsinki\/([a-z]+)$/.exec(path)?.[1];
  if (plural && PROJECT_LISTS[plural]) {
    return jsonResponse(list(PROJECT_LISTS[plural]));
  }
  if (path === "/api/v1/tools/generate") {
    return jsonResponse({
      jsonSchema: {
        definitions: {
          AirQualityObserved: { type: "object", properties: { pm10: { type: "number", title: "pm10" } } },
          Station: { type: "object", properties: { name: { type: "string", title: "name" } } },
        },
      },
      example: { type: "AirQualityObserved", pm10: 31.4 },
    });
  }
  return undefined;
}

/** A viewer: may read models, may not propose one. */
const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the list of data models", () => {
  it("lists every model with its space, version, lifecycle, classes, last change and users", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer });
    const table = await screen.findByRole("table", { name: en.models.page.listCaption });
    const air = (await within(table).findByRole("link", { name: "Air quality" })).closest("tr") as HTMLElement;
    expect(within(table).getByRole("link", { name: "Air quality" })).toHaveAttribute("href", "/projects/helsinki/models/air");
    expect(within(air).getByRole("link", { name: "ilma" })).toHaveAttribute("href", "/projects/helsinki/spaces/ilma");
    expect(air).toHaveTextContent("1.2.0");
    expect(air).toHaveTextContent(en.models.lifecycleOption.published);
    expect(air).toHaveTextContent("AirQualityObserved, Station");
    expect(air).toHaveTextContent("2 endpoints, 1 pipeline, 1 app, 1 dashboard");
    const events = within(table).getByRole("link", { name: "events" }).closest("tr") as HTMLElement;
    expect(events).toHaveTextContent(en.models.page.unused);
    expect(within(table).getAllByRole("row")).toHaveLength(4);
  });

  // T-6254 judge: a model of 27 classes filled its Types cell with a four-line wall.
  it("names at most TYPES_SHOWN types in a row and links the rest as +N", async () => {
    const many = Array.from({ length: TYPES_SHOWN + 3 }, (_, index) => `Type${index + 1}`);
    const wide = manifest("DataModel", "city", { contextSpaceRef: "ilma", linkml: "./city.linkml.yaml", version: "1.0.0", classes: many });
    await renderRoute({
      path: "/projects/helsinki/models",
      answer: (path) => (path === "/api/v1/projects/helsinki/datamodels" ? jsonResponse(list([wide])) : answer(path)),
    });
    const table = await screen.findByRole("table", { name: en.models.page.listCaption });
    const row = (await within(table).findByRole("link", { name: "city" })).closest("tr") as HTMLElement;
    expect(row).toHaveTextContent(`${many.slice(0, TYPES_SHOWN).join(", ")}, +3`);
    expect(within(row).queryByRole("link", { name: many[TYPES_SHOWN] })).not.toBeInTheDocument();
    const more = within(row).getByRole("link", { name: en.models.page.moreTypes.replace("{count}", "3").replace("{model}", "city") });
    expect(more).toHaveAttribute("href", "/projects/helsinki/models/city");
  });

  it("narrows by the search and by the space", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer });
    const table = await screen.findByRole("table", { name: en.models.page.listCaption });
    await within(table).findByRole("link", { name: "Air quality" });
    await userEvent.type(screen.getByLabelText(en.models.page.search), "bikehire");
    expect(within(table).getAllByRole("link").map((link) => link.textContent)).toEqual(["bikes", "mobility", "BikeHireDockingStation"]);
    await userEvent.clear(screen.getByLabelText(en.models.page.search));
    // The space is a filter chip of the shared grid (T-6115).
    const projectGrid = table.closest<HTMLElement>("[data-manifest-grid]")!;
    await userEvent.click(within(projectGrid).getByText(en.grid.filter));
    await userEvent.click(screen.getByRole("checkbox", { name: /^culture/ }));
    expect(within(table).getAllByRole("link").map((link) => link.textContent)).toEqual(["events", "culture"]);
  });

  it("opens the new model's window straight from New when there is none", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer: (path) => (path.endsWith("/datamodels") ? jsonResponse(list([])) : answer(path)) });
    expect(await screen.findByText(en.models.page.emptyTitle)).toBeInTheDocument();
    // No menu first (owner 2026-11-01): New goes to the window that names the model.
    const links = screen.getAllByRole("link", { name: en.models.page.new.menu });
    for (const link of links) expect(link).toHaveAttribute("href", "/projects/helsinki/models?new=file");
    expect(screen.queryByRole("menuitem")).toBeNull();
  });

  it("gives a person who may not propose a model the reason instead of the way", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer, permissions: READER });
    await screen.findByRole("table", { name: en.models.page.listCaption });
    await waitFor(() => expect(screen.getByRole("button", { name: en.models.page.new.menu })).toHaveAttribute("aria-disabled", "true"));
  });

  it("opens the editor when the address asks for one, after the new model's name", async () => {
    await renderRoute({ path: "/projects/helsinki/models?new=blank", answer });
    const dialog = await screen.findByRole("dialog", { name: en.models.newDialog.title });
    await userEvent.type(within(dialog).getByRole("textbox", { name: en.models.identity.name }), "trams");
    await userEvent.click(within(dialog).getByRole("button", { name: en.models.newDialog.create }));
    expect(await screen.findByRole("tab", { name: en.models.view.structure, selected: true })).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: en.models.page.listCaption })).toBeNull();
  });

  // New data model is one window for the name (owner 2026-11-01, T-6344); types then come in from
  // the editor's Add type, a Smart Data Model among them.
  it("names a new model in its window, then brings a Smart Data Model in from Add type", async () => {
    await renderRoute({ path: "/projects/helsinki/models?new=sdm", answer });
    const dialog = await screen.findByRole("dialog", { name: en.models.newDialog.title });
    const create = within(dialog).getByRole("button", { name: en.models.newDialog.create });
    expect(create).toHaveAttribute("aria-disabled", "true");
    // The wait is said where a sighted person reads it, and goes once a name is typed.
    expect(within(dialog).getByText(en.models.nameFirst, { selector: "p" })).toBeVisible();
    await userEvent.type(within(dialog).getByRole("textbox", { name: en.models.identity.name }), "trams");
    expect(within(dialog).queryByText(en.models.nameFirst, { selector: "p" })).toBeNull();
    await userEvent.click(create);
    expect(screen.queryByRole("dialog", { name: en.models.newDialog.title })).toBeNull();
    expect(await screen.findByRole("tab", { name: en.models.view.structure, selected: true })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.models.addClass }));
    await userEvent.click(within(screen.getByRole("dialog", { name: en.models.addClass })).getByRole("button", { name: new RegExp(en.models.addFrom.sdm) }));
    expect(await screen.findByRole("dialog", { name: en.models.importDialog.title })).toBeInTheDocument();
  });
});

/** The organization's list (DM-63, DM-79): one organization model and one of this project. */
const ORGANIZATION = {
  items: [
    { name: "stations", level: "organization", project: "org", version: "2.1.0", lifecycle: "published", classes: ["Station", "Dock"] },
    { name: "air", level: "project", project: "helsinki", space: "ilma", version: "1.2.0", lifecycle: "published", classes: ["Station"] },
  ],
  smartDataModels: [],
};

const withOrganization =
  (organization: unknown) =>
  (path: string): Response | undefined =>
    path === "/api/v1/organization/datamodels" ? jsonResponse(organization) : answer(path);

describe("organization models beside the project's (T-2883, DM-75, DM-79)", () => {
  it("shows two sections, the organization's read-only with the name a model imports it by", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer: withOrganization(ORGANIZATION) });
    expect(await screen.findByRole("region", { name: en.models.page.projectSection })).toBeInTheDocument();
    const section = await screen.findByRole("region", { name: en.models.page.organizationSection });
    const table = await within(section).findByRole("table", { name: en.models.page.organizationCaption });
    await within(table).findByText("stations");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent("stations");
    expect(rows[0]).toHaveTextContent("Station, Dock");
    expect(rows[0]).toHaveTextContent("org.stations.v2");
    // Read-only: the shared grid's search, sort and details, never a write.
    expect(within(section).queryByRole("button", { name: /propose|edit|delete/i })).toBeNull();
    expect(within(section).getByRole("link", { name: en.models.page.organizationOpen })).toHaveAttribute(
      "href",
      "/organization/models",
    );
  });

  it("says the organization shares none when the list holds no organization model", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer: withOrganization({ ...ORGANIZATION, items: [ORGANIZATION.items[1]] }) });
    const section = await screen.findByRole("region", { name: en.models.page.organizationSection });
    expect(await within(section).findByText(en.models.page.organizationEmpty)).toBeInTheDocument();
    expect(within(section).queryByRole("table")).toBeNull();
  });

  it("gives the reason and a retry when the organization's list cannot be read", async () => {
    await renderRoute({
      path: "/projects/helsinki/models",
      answer: (path) =>
        path === "/api/v1/organization/datamodels"
          ? jsonResponse({ title: "Unavailable", status: 503, detail: "the mirror is loading" }, 503)
          : answer(path),
    });
    const section = await screen.findByRole("region", { name: en.models.page.organizationSection });
    expect(await within(section).findByText(/the mirror is loading/)).toBeInTheDocument();
    // The project's own models are unaffected.
    expect(await screen.findByRole("table", { name: en.models.page.listCaption })).toBeInTheDocument();
  });

  it("searches, sorts by the number of types and filters by lifecycle on the shared grid (T-6205)", async () => {
    const items = [
      ORGANIZATION.items[0],
      { name: "parks", level: "organization", project: "org", version: "0.2.0", lifecycle: "draft", classes: ["Park"] },
      { name: "transit", level: "organization", project: "org", version: "1.0.0", lifecycle: "published", classes: ["Stop", "Line", "Trip"] },
    ];
    await renderRoute({ path: "/organization/models", answer: withOrganization({ ...ORGANIZATION, items }) });
    const table = await screen.findByRole("table", { name: en.models.page.organizationCaption });
    await within(table).findByText("parks");
    await narrowsAndOrders(table, {
      search: { label: `Search ${en.models.page.organizationSection}`, text: "trip", kept: ["transit"] },
      sort: { column: en.models.page.classCount, ascending: ["parks", "stations", "transit"] },
      filter: { value: /^Draft/, kept: ["parks"] },
    });
  });

  it("names the import by the major version alone", () => {
    expect(organizationImportName("stations", "2.1.0")).toBe("org.stations.v2");
    expect(organizationImportName("air", "0.3.1")).toBe("org.air.v0");
  });

  it("lists them on the Organization page's Data models tab", async () => {
    await renderRoute({ path: "/organization/models", answer: withOrganization(ORGANIZATION) });
    expect(await screen.findByRole("link", { name: en.organization.tab.models, current: "page" })).toBeInTheDocument();
    const table = await screen.findByRole("table", { name: en.models.page.organizationCaption });
    expect(await within(table).findByText("org.stations.v2")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: en.models.page.organizationOpen })).toBeNull();
  });
});

describe("a model's own page", () => {
  it("shows the overview: the diagram, the classes with what each slot is, and the enums", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    expect(await screen.findByRole("heading", { name: "Air quality", level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: en.models.graph.title })).toBeInTheDocument();
    // Each type a table of its fields in plain words (T-4829), never a range name.
    const fields = screen.getByRole("table", { name: "Fields of AirQualityObserved" });
    expect(within(fields).getByText("refStation").closest("tr")).toHaveTextContent("refStationLink to Station");
    expect(within(fields).getByText("level").closest("tr")).toHaveTextContent(`level${en.spaces.fields.types.select}`);
    expect(within(fields).getByText("pm10").closest("tr")).toHaveTextContent(`pm10${en.spaces.fields.types.number} · ${en.models.required}`);
    expect(screen.getByRole("button", { name: "Add field to AirQualityObserved" })).toBeInTheDocument();
    expect(screen.getByText(/good \(Good\), bad/)).toBeInTheDocument();
    // The breadcrumb leads back to the list (owner 2026-10-31: no second "All data models" link).
    expect(within(screen.getByRole("navigation", { name: en.nav.breadcrumb })).getByRole("link", { name: en.nav.models })).toHaveAttribute("href", "/projects/helsinki/models");
    expect(screen.queryByRole("link", { name: en.models.page.back })).toBeNull();
    expect(screen.getByRole("link", { name: en.models.page.edit })).toHaveAttribute("href", "/projects/helsinki/models?edit=air");
  });

  it("ends the one toolbar row with Open full screen, and names no space in the header (T-6146)", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    const toolbar = await screen.findByRole("toolbar", { name: en.models.graph.tools });
    const open = within(toolbar).getByRole("link", { name: en.models.canvas.open });
    expect(open).toHaveAttribute("href", "/projects/helsinki/models/air/diagram");
    const focusable = within(toolbar).getAllByRole("link").concat(within(toolbar).getAllByRole("button"));
    expect(focusable.every((one) => one === open || one.compareDocumentPosition(open) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true);
    // A DataModel binds no space (T-6101): "Where used" says where it is used, the header does not.
    const header = screen.getByRole("heading", { name: "Air quality", level: 1 }).parentElement?.parentElement;
    expect(header).not.toHaveTextContent(en.models.field.space);
    expect(screen.queryByRole("link", { name: "ilma" })).not.toBeInTheDocument();
  });

  it("shows the form a class generates, filled with the example", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    await userEvent.click(await screen.findByRole("tab", { name: en.models.page.view.form }));
    expect(await screen.findByDisplayValue("31.4")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText(en.models.page.formClass), "Station");
    expect(await screen.findByLabelText(/name/)).toBeInTheDocument();
    expect(screen.queryByDisplayValue("31.4")).toBeNull();
  });

  it("shows the YAML as text, with copy and download", async () => {
    await renderRoute({
      path: "/projects/helsinki/models/air",
      answer: (path) =>
        path.endsWith("/datamodels")
          ? jsonResponse(list([{ ...MODELS[0], spec: { ...MODELS[0].spec, linkml: `${AIR}# <b>not markup</b>\n` } }]))
          : answer(path),
    });
    await userEvent.click(await screen.findByRole("tab", { name: en.models.page.view.yaml }));
    const yaml = screen.getByLabelText(en.models.page.yaml);
    expect(yaml).toHaveTextContent("# <b>not markup</b>");
    expect(yaml.querySelector("b")).toBeNull();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await userEvent.click(screen.getByRole("button", { name: en.models.page.copyYaml }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("name: air"));
    expect(await screen.findByText(en.models.page.copied)).toBeInTheDocument();
  });

  it("lists where the model is used, each a link", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    await userEvent.click(await screen.findByRole("tab", { name: en.models.page.view.used }));
    const used = within(screen.getByRole("tabpanel"));
    expect(used.getByRole("link", { name: "ilma" })).toHaveAttribute("href", "/projects/helsinki/spaces/ilma");
    expect(used.getByRole("link", { name: "air-public" })).toHaveAttribute("href", "/projects/helsinki/endpoints/air-public");
    expect(used.getByRole("link", { name: "air-ingest" })).toHaveAttribute("href", "/projects/helsinki/pipelines/air-ingest");
    expect(used.getByRole("link", { name: "city" })).toHaveAttribute("href", "/projects/helsinki/dashboards/city/edit");
    expect(used.queryByRole("link", { name: "bikes-public" })).toBeNull();
  });

  it("reads its history as the model's closed Changes (T-4830)", async () => {
    const { calls, fetchMock } = await renderRoute({ path: "/projects/helsinki/models/air", answer });
    await userEvent.click(await screen.findByRole("tab", { name: en.models.page.view.history }));
    await waitFor(() => expect(calls()).toContain("GET /api/v1/projects/helsinki/changes/history"));
    const history = fetchMock.mock.calls
      .map((call) => (call[0] as Request).url)
      .find((url) => url.includes("/changes/history"));
    const query = new URL(history ?? "").searchParams;
    expect([query.get("kind"), query.get("name")]).toEqual(["DataModel", "air"]);
  });

  it("says so for a model the project does not have", async () => {
    await renderRoute({ path: "/projects/helsinki/models/nope", answer });
    expect(await screen.findByText(en.models.page.notFound.replace("{name}", "nope"))).toBeInTheDocument();
  });

  it("opens the space's Add field dialog on a type, which proposes and never writes", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer });
    await userEvent.click(await screen.findByRole("button", { name: "Add field to Station" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("button", { name: en.spaces.fields.propose })).toBeInTheDocument();
    expect(within(dialog).queryByText(/IRI/)).toBeNull();
  });

  it("gives a viewer the types read-only: no Add field, no Add type, nothing disabled", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer, permissions: READER });
    expect(await screen.findByRole("table", { name: "Fields of Station" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Add field to/ })).toBeNull();
    expect(screen.queryByRole("link", { name: en.models.page.addType })).toBeNull();
    expect(screen.queryByRole("button", { name: en.models.page.addType })).toBeNull();
  });

  it("sends the old /datamodels addresses to the one model editor", async () => {
    await renderRoute({ path: "/projects/helsinki/datamodels/air/edit", answer });
    await waitFor(() => expect(`${window.location.pathname}${window.location.search}`).toBe("/projects/helsinki/models?edit=air"));
    await renderRoute({ path: "/projects/helsinki/datamodels/air", answer });
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/models/air"));
    await renderRoute({ path: "/projects/helsinki/datamodels", answer });
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/models"));
  });

  it("disables Edit, with the reason, for a person who may not propose a model", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air", answer, permissions: READER });
    const edit = await screen.findByRole("button", { name: en.models.page.edit });
    expect(edit).toHaveAttribute("aria-disabled", "true");
  });
});

describe("what uses a model", () => {
  it("follows the space to its endpoints, and the endpoints and types to the rest", () => {
    const uses = usesOfModel(MODELS[0], PROJECT_LISTS).map((use) => `${use.kind}/${use.name}`);
    expect(uses).toEqual([
      "ContextSpace/ilma",
      "Endpoint/air-internal",
      "Endpoint/air-public",
      "Pipeline/air-ingest",
      "Subscription/air-alerts",
      "App/air-map",
      "Dashboard/city",
      "Mapping/air-to-sdm",
    ]);
    expect(usesOfModel(MODELS[2], PROJECT_LISTS).map((use) => use.kind)).toEqual(["ContextSpace"]);
    // A model whose space the person may not read counts nothing through it.
    expect(usesOfModel(MODELS[0], {})).toEqual([]);
  });

  it("finds the newest change among the model's conditions", () => {
    expect(lastChangeOf(MODELS[0])).toBe("2026-09-20T08:00:00Z");
    expect(lastChangeOf(MODELS[1])).toBeUndefined();
  });

  it("splits a YAML line into key, value and comment", () => {
    expect(yamlParts("  pm10: { range: float } # measured")).toEqual({
      indent: "  ",
      key: "pm10:",
      rest: " { range: float }",
      comment: " # measured",
    });
    expect(yamlParts("    - linkml:types")).toEqual({ indent: "    - ", key: undefined, rest: "linkml:types" });
    expect(yamlParts("# only a comment")).toEqual({ indent: "", rest: "", comment: "# only a comment" });
  });
});

describe("a link to a user of the model", () => {
  it("opens a kind with a page of its own there, and anything else on its edit form", async () => {
    const { UseLink } = await import("../src/pages/models/ModelPage");
    const { createRootRoute, createRouter, RouterProvider } = await import("@tanstack/react-router");
    const { render } = await import("@testing-library/react");
    const root = createRootRoute({
      component: () => (
        <>
          <UseLink project="helsinki" use={{ plural: "apps", name: "air-map" }} />
          <UseLink project="helsinki" use={{ plural: "subscriptions", name: "air-alerts" }} />
        </>
      ),
    });
    render(<RouterProvider router={createRouter({ routeTree: root })} />);
    expect(await screen.findByRole("link", { name: "air-map" })).toHaveAttribute("href", "/projects/helsinki/apps/air-map");
    expect(screen.getByRole("link", { name: "air-alerts" })).toHaveAttribute("href", "/projects/helsinki/subscriptions/air-alerts/edit");
  });
});
