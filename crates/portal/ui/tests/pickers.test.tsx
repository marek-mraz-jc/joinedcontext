/**
 * The data model and type pickers (T-2701, ADR-N-033, DM-63): one list from
 * `/api/v1/organization/datamodels`, searched by typing, operated by keyboard, grouped by project
 * and space with the version, the catalogue offered on a search, a space's imported classes
 * grouped apart from its own, "create new" where the form allows it, and a failed list said as one.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import i18n from "../src/i18n";
import { DataModelPicker } from "../src/components/pickers/DataModelPicker";
import type { ModelChoice } from "../src/components/pickers/DataModelPicker";
import { TypePicker, importedNames } from "../src/components/pickers/TypePicker";
import { Combobox } from "../src/components/pickers/Combobox";
import { ResourceNamePicker } from "../src/components/pickers/ResourceNamePicker";
import { catalogueValue, modelValue } from "../src/components/pickers/organizationModels";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { DataModelPickerWidget, TypePickerWidget } from "../src/components/forms/widgets/ModelWidgets";
import { portalWidgets } from "../src/components/forms/widgets";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { contextSpaceSchema, policySchema, policyUiSchema } from "../src/schemas/kinds";
import { mappingSchema, mappingUiSchema } from "../src/schemas/mapping";

const MODELS = [
  { name: "air-quality", level: "project", project: "helsinki", space: "air", version: "1.2.0", lifecycle: "published", classes: ["AirQualityObserved"] },
  { name: "mobility", level: "project", project: "helsinki", space: "mobility", version: "0.3.0", lifecycle: "draft", classes: ["BikeHireDockingStation", "Road"] },
  { name: "kpi", level: "project", project: "bbsk", space: "kpi", version: "2.0.0", lifecycle: "published", classes: ["KeyPerformanceIndicator"] },
];
/** An organization model named like a project's, and a project model no space owns (DM-75). */
const LEVELS = [
  { name: "kpi", level: "organization", project: "org", version: "2.1.0", lifecycle: "published", classes: ["Territory"] },
  { name: "shared", level: "project", project: "helsinki", version: "1.0.0", lifecycle: "published", classes: ["Kiosk"] },
];
let extra: typeof LEVELS = [];
const SDM = [
  { id: "dataModel.Environment/AirQualityObserved", name: "AirQualityObserved", subject: "dataModel.Environment", description: "Air quality" },
];

let requests: string[] = [];
let failList = false;
const SOURCE = "imports:\n  - linkml:types\n  - ../kpi/kpi.linkml.yaml\nclasses: {}\n";
let source = SOURCE;

function stubFetch() {
  requests = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
      requests.push(url.pathname + url.search);
      if (url.pathname === "/api/v1/organization/datamodels") {
        if (failList) {
          return new Response(JSON.stringify({ title: "Forbidden", status: 500, detail: "the mirror is not ready" }), {
            status: 500,
            headers: { "Content-Type": "application/problem+json" },
          });
        }
        const search = url.searchParams.get("search") ?? "";
        return Response.json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [...extra, ...MODELS],
          smartDataModels: search.length >= 2 ? SDM.filter((e) => e.name.toLowerCase().includes(search.toLowerCase())) : [],
        });
      }
      if (url.pathname === "/api/v1/projects/helsinki/datamodels/mobility/source") {
        return new Response(source, { headers: { "Content-Type": "text/yaml" } });
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

function wrap(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  failList = false;
  extra = [];
  source = SOURCE;
  await i18n.changeLanguage("en");
  stubFetch();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("DataModelPicker", () => {
  it("lists every model the server returned, own project first, then the organization's, grouped by level, project and space with the version", async () => {
    const user = userEvent.setup();
    extra = LEVELS;
    wrap(<DataModelPicker label="Data model" project="bbsk" value={[]} onChange={() => {}} />);
    await user.click(screen.getByRole("combobox", { name: "Data model" }));
    const list = await screen.findByRole("listbox", { name: "Data model" });
    const options = await within(list).findAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      expect.stringContaining("KeyPerformanceIndicator"),
      expect.stringContaining("Territory"),
      expect.stringContaining("shared"),
      expect.stringContaining("air-quality"),
      expect.stringContaining("mobility"),
    ]);
    // DM-79: each entry's level is said, the organization's models apart from every project's.
    expect(list.textContent).toContain("Project bbsk / kpi");
    expect(list.textContent).toContain("Organization models");
    expect(list.textContent).toContain("Project helsinki / air");
    expect(list.textContent).toContain("Project helsinki");
    expect(options[3].textContent).toContain("v1.2.0");
    expect(options[4].textContent).toContain("v0.3.0 · draft");
  });

  it("is operated by keyboard: arrows move, Enter picks, Escape closes", async () => {
    const user = userEvent.setup();
    const picked: [string[], ModelChoice[]][] = [];
    wrap(<DataModelPicker label="Data model" project="helsinki" value={[]} onChange={(v, c) => picked.push([v, c])} />);
    const box = screen.getByRole("combobox", { name: "Data model" });
    box.focus();
    await screen.findAllByRole("option");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(picked.at(-1)?.[0]).toEqual(["helsinki/mobility"]);
    const choice = picked.at(-1)?.[1][0];
    expect(choice?.source === "organization" && choice.model.space).toBe("mobility");
    expect(screen.queryByRole("listbox")).toBeNull();
    await user.keyboard("{ArrowDown}");
    expect(await screen.findByRole("listbox")).toBeTruthy();
    expect(box.getAttribute("aria-activedescendant")).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("searches the catalogue from two characters on and says one character does not", async () => {
    const user = userEvent.setup();
    const picked: ModelChoice[][] = [];
    wrap(<DataModelPicker label="Data model" value={[]} onChange={(_, c) => picked.push(c)} />);
    await user.click(screen.getByRole("combobox", { name: "Data model" }));
    expect(await screen.findByText("Type two letters or more to search Smart Data Models too.")).toBeTruthy();
    await user.type(screen.getByRole("combobox"), "air");
    const entry = await screen.findByRole("option", { name: /AirQualityObserved.*Air quality/ });
    expect(requests.some((r) => r.includes("search=air"))).toBe(true);
    expect(requests.some((r) => r.includes("search=a&") || r.endsWith("search=a"))).toBe(false);
    await user.click(entry);
    expect(picked.at(-1)?.[0]).toEqual({ source: "catalogue", entry: SDM[0] });
  });

  it("says when nothing matches and offers a new model only where the form allows one", async () => {
    const user = userEvent.setup();
    const created: string[] = [];
    const { unmount } = wrap(<DataModelPicker label="Data model" catalogue={false} value={[]} onChange={() => {}} />);
    await user.type(screen.getByRole("combobox"), "parking");
    expect(await screen.findByText("No data model matches.")).toBeTruthy();
    unmount();
    wrap(<DataModelPicker label="Data model" catalogue={false} value={[]} onChange={() => {}} onCreate={(n) => created.push(n)} />);
    await user.type(screen.getByRole("combobox"), "parking");
    await user.keyboard("{Enter}");
    expect(created).toEqual(["parking"]);
  });

  it("says a failed list is a failure, with a retry, and not an empty organization", async () => {
    failList = true;
    const user = userEvent.setup();
    wrap(<DataModelPicker label="Data model" value={[]} onChange={() => {}} />);
    await user.click(screen.getByRole("combobox"));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("the mirror is not ready");
    failList = false;
    await user.click(within(alert).getByRole("button"));
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(3));
  });
});

describe("TypePicker", () => {
  it("lists the space's own classes and, apart, the classes of the models it imports", async () => {
    const user = userEvent.setup();
    wrap(<TypePicker label="Type" project="helsinki" space="mobility" value={[]} onChange={() => {}} />);
    await user.click(screen.getByRole("combobox", { name: "Type" }));
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(3));
    const list = screen.getByRole("listbox");
    expect(list.textContent).toContain("Types of mobility");
    expect(list.textContent).toContain("Imported from bbsk/kpi");
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      expect.stringContaining("BikeHireDockingStation"),
      expect.stringContaining("Road"),
      expect.stringContaining("KeyPerformanceIndicator"),
    ]);
    expect(list.textContent).not.toContain("AirQualityObserved");
  });

  it("finds an organization model the space imports by its level, not a project model of the same name", async () => {
    source = "imports:\n  - linkml:types\n  - org.kpi.v2\nclasses: {}\n";
    extra = LEVELS;
    const user = userEvent.setup();
    wrap(<TypePicker label="Type" project="helsinki" space="mobility" value={[]} onChange={() => {}} />);
    await user.click(screen.getByRole("combobox", { name: "Type" }));
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(3));
    const list = screen.getByRole("listbox");
    expect(list.textContent).toContain("Imported from the organization model kpi");
    expect(list.textContent).toContain("Territory");
    expect(list.textContent).not.toContain("KeyPerformanceIndicator");
  });

  it("takes several types as removable pills", async () => {
    const user = userEvent.setup();
    let value: string[] = ["Road"];
    const { rerender } = wrap(
      <TypePicker label="Type" project="helsinki" space="mobility" multiple value={value} onChange={(v) => (value = v)} />,
    );
    await user.click(screen.getByRole("combobox", { name: "Type" }));
    await user.click(await screen.findByRole("option", { name: /BikeHireDockingStation/ }));
    expect(value).toEqual(["Road", "BikeHireDockingStation"]);
    rerender(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient()}>
          <TypePicker label="Type" project="helsinki" space="mobility" multiple value={value} onChange={(v) => (value = v)} />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    await user.click(screen.getByRole("button", { name: "Remove Road" }));
    expect(value).toEqual(["BikeHireDockingStation"]);
  });

  it("without a space lists every model of the project, grouped by model, and nothing of another project", async () => {
    const user = userEvent.setup();
    extra = LEVELS;
    wrap(<TypePicker label="Type" project="helsinki" value={[]} onChange={() => {}} />);
    await user.click(screen.getByRole("combobox"));
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(4));
    const list = screen.getByRole("listbox");
    expect(list.textContent).not.toContain("KeyPerformanceIndicator");
    expect(list.textContent).toContain("shared (project model)");
    expect(list.textContent).not.toContain("undefined");
    expect(requests.some((r) => r.endsWith("/source"))).toBe(false);
  });

  it("says a space whose model declares nothing is empty", async () => {
    const user = userEvent.setup();
    wrap(<TypePicker label="Type" project="helsinki" space="parking" value={[]} onChange={() => {}} />);
    await user.click(screen.getByRole("combobox"));
    expect(await screen.findByText("The model of parking declares no type yet.")).toBeTruthy();
  });
});

describe("importedNames", () => {
  it("reads model names from imports and skips LinkML built-ins and unreadable sources", () => {
    expect(importedNames("imports:\n  - linkml:types\n  - ../air/air-quality.linkml.yaml\n  - https://example.org/models/kpi.yaml\n")).toEqual([
      { name: "air-quality" },
      { name: "kpi" },
    ]);
    // DM-76: a platform-model import carries its level; a malformed one is only a name.
    expect(importedNames("imports: [org.kpi.v2, project.air-quality.v1, org.bad]\n")).toEqual([
      { name: "kpi", level: "organization" },
      { name: "air-quality", level: "project" },
      { name: "org.bad" },
    ]);
    expect(importedNames("classes: {}\n")).toEqual([]);
    expect(importedNames(": : not yaml [")).toEqual([]);
    source = "imports: linkml:types\n";
    expect(importedNames(source)).toEqual([]);
  });
});

describe("Combobox", () => {
  const OPTIONS = ["alpha", "beta", "gamma"].map((value) => ({ value, label: value, group: "Letters" }));

  it("moves to the first and last option with Home and End, and Backspace removes the last pill", async () => {
    const user = userEvent.setup();
    let value = ["alpha", "beta"];
    wrap(<Combobox label="Letters" multiple value={value} options={OPTIONS} empty="none" onChange={(v) => (value = v)} />);
    const box = screen.getByRole("combobox", { name: "Letters" });
    box.focus();
    await user.keyboard("{End}");
    expect(box.getAttribute("aria-activedescendant")).toMatch(/-2$/);
    await user.keyboard("{Home}");
    expect(box.getAttribute("aria-activedescendant")).toMatch(/-0$/);
    expect(screen.getByRole("listbox").getAttribute("aria-multiselectable")).toBe("true");
    expect(screen.getAllByRole("option", { selected: true })).toHaveLength(2);
    await user.keyboard("{Backspace}");
    expect(value).toEqual(["alpha"]);
  });

  it("shows nothing chosen and no list while disabled", async () => {
    const user = userEvent.setup();
    wrap(<Combobox label="Letters" disabled value={[]} options={OPTIONS} empty="none" onChange={() => {}} />);
    await user.click(screen.getByRole("combobox", { name: "Letters" }));
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("picker values", () => {
  it("names a model by project and name and a catalogue entry by its id", () => {
    expect(modelValue({ project: "helsinki", name: "air-quality" })).toBe("helsinki/air-quality");
    expect(catalogueValue({ id: "dataModel.Environment/AirQualityObserved" })).toBe("sdm:dataModel.Environment/AirQualityObserved");
  });
});

describe("the forms name models and types through the pickers (T-2701)", () => {
  it("registers both pickers under the names a uiSchema writes", () => {
    expect(portalWidgets.typePicker).toBe(TypePickerWidget);
    expect(portalWidgets.dataModelPicker).toBe(DataModelPickerWidget);
  });

  function form(kind: string, schema: JsonSchema, uiSchema: UiSchema, formData: Record<string, unknown>) {
    return wrap(
      <SchemaForm kind={kind} project="helsinki" schema={schema} uiSchema={uiSchema} formData={formData} onSubmit={() => {}} />,
    );
  }

  it("a Policy's entity type lists the classes of the policy's space", async () => {
    const user = userEvent.setup();
    form("Policy", policySchema(i18n.t.bind(i18n), ["air", "mobility"]), policyUiSchema, {
      contextSpaceRef: "mobility",
      information: [{ entities: [{}] }],
    });
    const box = screen.getByRole("combobox", { name: new RegExp(i18n.t("policies.field.entityType")) });
    await user.click(box);
    await waitFor(() => expect(within(screen.getByRole("listbox")).getAllByRole("option").map((o) => o.textContent)).toEqual([
      expect.stringContaining("BikeHireDockingStation"),
      expect.stringContaining("Road"),
      expect.stringContaining("KeyPerformanceIndicator"),
    ]));
  });

  it("a space's data model and a mapping's source are the project's models, picked by name", async () => {
    const user = userEvent.setup();
    const changed: unknown[] = [];
    const { unmount } = wrap(
      <SchemaForm
        kind="ContextSpace"
        project="helsinki"
        schema={contextSpaceSchema(i18n.t.bind(i18n))}
        formData={{ name: "air" }}
        onSubmit={() => {}}
        onChange={(data) => changed.push(data)}
      />,
    );
    await user.click(screen.getByRole("combobox", { name: new RegExp(i18n.t("spaces.field.dataModel")) }));
    const options = await within(await screen.findByRole("listbox")).findAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([expect.stringContaining("air-quality"), expect.stringContaining("mobility")]);
    await user.click(options[0]);
    expect(changed.at(-1)).toMatchObject({ dataModelRef: "air-quality" });
    unmount();

    form("Mapping", mappingSchema(i18n.t.bind(i18n)), mappingUiSchema, { source: { name: "mobility" } });
    const [picked] = screen.getAllByRole("combobox", { name: i18n.t("mappings.field.model") });
    await waitFor(() => expect((picked as HTMLInputElement).value).toBe("mobility"));
    // The space is one of the project's spaces, from the list and not typed (T-2702).
    expect(screen.getByRole("combobox", { name: new RegExp(i18n.t("mappings.field.space")) }).tagName).toBe("SELECT");
  });
});

describe("ResourceNamePicker (T-2702)", () => {
  function stubList(items: string[]) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
        requests.push(url.pathname);
        if (url.pathname === "/api/v1/projects") {
          return Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: items.map((name) => ({ name })) });
        }
        return Response.json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          metadata: {},
          items: items.map((name) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name, namespace: "helsinki", title: { en: `${name} title` } }, spec: {} })),
        });
      }),
    );
  }

  it("lists a project's manifests by title and hands back the name", async () => {
    stubList(["air", "mobility"]);
    const user = userEvent.setup();
    const picked: string[] = [];
    wrap(<ResourceNamePicker label="Space" from={{ project: "helsinki", plural: "spaces" }} value="" onChange={(n) => picked.push(n)} />);
    await user.click(screen.getByRole("combobox", { name: "Space" }));
    await user.click(await screen.findByRole("option", { name: /mobility title/ }));
    expect(picked).toEqual(["mobility"]);
    expect(requests).toContain("/api/v1/projects/helsinki/spaces");
  });

  it("offers a new name only where the form creates one", async () => {
    stubList(["air"]);
    const user = userEvent.setup();
    const picked: string[] = [];
    const { unmount } = wrap(<ResourceNamePicker label="Space" from={{ project: "helsinki", plural: "spaces" }} value="" onChange={(n) => picked.push(n)} />);
    await user.type(screen.getByRole("combobox", { name: "Space" }), "parking");
    expect(await screen.findByText("No results")).toBeTruthy();
    unmount();
    wrap(<ResourceNamePicker label="Space" create from={{ project: "helsinki", plural: "spaces" }} value="" onChange={(n) => picked.push(n)} />);
    await user.type(screen.getByRole("combobox", { name: "Space" }), "parking");
    await user.click(await screen.findByRole("option", { name: "New: “parking”" }));
    expect(picked).toEqual(["parking"]);
  });

  it("lists the projects the caller reads", async () => {
    stubList(["helsinki", "espoo"]);
    const user = userEvent.setup();
    wrap(<ResourceNamePicker label="Project" from="projects" value="helsinki" onChange={() => {}} />);
    expect((screen.getByRole("combobox", { name: "Project" }) as HTMLInputElement).value).toBe("helsinki");
    await user.click(screen.getByRole("combobox", { name: "Project" }));
    await waitFor(() => expect(within(screen.getByRole("listbox")).getAllByRole("option")).toHaveLength(2));
  });
});
