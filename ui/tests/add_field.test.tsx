/**
 * "Add field" (T-3098, ADR-N-042 §3.1): each field type is a LinkML range and an NGSI-LD kind, and
 * adding one proposes a DataModel Change; the Smart Data Model of the same name is offered first.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AddFieldDialog } from "../src/components/entities/AddFieldDialog";
import { EMPTY_DRAFT, fieldOperations, formulaProblem, problemsOf, suggestedSlots, withFormula } from "../src/components/entities/fieldTypes";
import { parseModel } from "../src/pages/models/linkml";
import { applyOperations } from "../src/pages/models/operations";

const STATIONS = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
classes:
  BikeHireDockingStation:
    slots: [name]
  Street:
    slots: [label]
slots:
  name:
    range: string
  label:
    range: string
`;

const OFFICIAL = `id: https://smartdatamodels.org/BikeHireDockingStation
name: BikeHireDockingStation
enums:
  StatusEnum:
    permissible_values:
      working: {}
      outOfService: {}
classes:
  BikeHireDockingStation:
    slots: [name, availableBikeNumber, status, location, refStreet]
slots:
  name: { range: string }
  availableBikeNumber:
    range: integer
    slot_uri: https://smartdatamodels.org/dataModel.Transportation/availableBikeNumber
    description: Number of bikes available
    annotations: { upstream_source: "https://smartdatamodels.org/dataModel.Transportation/BikeHireDockingStation" }
  status: { range: StatusEnum }
  location:
    annotations: { ngsi_ld_kind: GeoProperty }
  refStreet:
    range: Street
    annotations: { ngsi_ld_kind: Relationship }
`;

const model = parseModel(STATIONS);
const TYPE = "BikeHireDockingStation";

function applied(draft: Partial<typeof EMPTY_DRAFT>) {
  const full = { ...EMPTY_DRAFT, ...draft };
  expect(problemsOf(full, model, TYPE)).toEqual({});
  const result = applyOperations(STATIONS, fieldOperations(full, model, TYPE));
  expect(result.refused).toEqual([]);
  return parseModel(result.source);
}

describe("field types", () => {
  it("map onto the LinkML range and NGSI-LD kind the field holds", () => {
    expect(applied({ name: "capacity", type: "integer", required: true }).slots.find((s) => s.name === "capacity")).toMatchObject({
      range: "integer",
      required: true,
      kind: "Property",
    });
    expect(applied({ name: "opened", type: "date" }).slots.find((s) => s.name === "opened")?.range).toBe("date");
    expect(applied({ name: "contact", type: "email" }).slots.find((s) => s.name === "contact")?.pattern).toBeDefined();
    expect(applied({ name: "where", type: "location" }).slots.find((s) => s.name === "where")?.kind).toBe("GeoProperty");
    const after = applied({ name: "attached", type: "multiSelect", values: ["new", "in progress"] });
    const slot = after.slots.find((s) => s.name === "attached")!;
    expect(slot.multivalued).toBe(true);
    expect(after.enums.find((e) => e.name === slot.range)?.permissible_values.map((v) => v.name)).toEqual(["new", "in progress"]);
    expect(after.classes.find((c) => c.name === TYPE)?.slots).toContain("attached");
  });

  it("makes a relationship with its way back on the target", () => {
    const after = applied({ name: "street", type: "relationship", target: "Street", inverse: "stations" });
    expect(after.slots.find((s) => s.name === "street")).toMatchObject({ range: "Street", kind: "Relationship" });
    expect(after.slots.find((s) => s.name === "stations")?.multivalued).toBe(true);
  });

  it("refuses a name the type has, a bad name, an empty choice list and a missing target", () => {
    expect(problemsOf({ ...EMPTY_DRAFT, name: "name" }, model, TYPE).name).toBe("nameTaken");
    expect(problemsOf({ ...EMPTY_DRAFT, name: "kapacita stanice" }, model, TYPE).name).toBe("nameInvalid");
    expect(problemsOf({ ...EMPTY_DRAFT, name: "s", type: "select" }, model, TYPE).values).toBe("valuesMissing");
    expect(problemsOf({ ...EMPTY_DRAFT, name: "s", type: "select", values: ["a", "a"] }, model, TYPE).values).toBe("valueTwice");
    const rel = problemsOf({ ...EMPTY_DRAFT, name: "r", type: "relationship", inverse: "r" }, model, TYPE);
    expect(rel).toEqual({ target: "targetMissing", inverse: "inverseInvalid" });
    expect(problemsOf({ ...EMPTY_DRAFT, name: "r", type: "relationship", target: "Street", inverse: "label" }, model, TYPE).inverse).toBe(
      "inverseTaken",
    );
  });

  it("makes a formula field a slot carrying its equals_expression and the range its value has (DM-80)", () => {
    const draft = { ...EMPTY_DRAFT, name: "title", type: "formula" as const, expression: "{name} + ' station'", required: true };
    expect(problemsOf(draft, model, TYPE)).toEqual({});
    const result = withFormula(applyOperations(STATIONS, fieldOperations(draft, model, TYPE)), draft.name, draft.expression);
    const slot = parseModel(result.source).slots.find((s) => s.name === "title");
    // Required means nothing for a value a pipeline computes, so it is not written.
    expect(slot).toMatchObject({ range: "string", equals_expression: "{name} + ' station'", required: false });
  });

  it("refuses a formula reading what the class does not have, or itself", () => {
    const draft = (expression: string) => ({ ...EMPTY_DRAFT, name: "spare", type: "formula" as const, expression });
    expect(problemsOf(draft("{capacity} - 1"), model, TYPE).expression).toBe("formulaInvalid");
    expect(formulaProblem(draft("{capacity} - 1"), model, TYPE)).toBe("{capacity} is not a slot of this class");
    expect(formulaProblem(draft("{spare} + 1"), model, TYPE)).toBe("a cycle: spare → spare");
    expect(formulaProblem(draft(""), model, TYPE)).toBe("the formula is empty");
  });

  it("suggests the official attributes the type lacks and a field can hold", () => {
    expect(suggestedSlots(parseModel(OFFICIAL), model, TYPE).map((s) => s.name)).toEqual(["availableBikeNumber", "status", "location"]);
  });
});

function respond(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

describe("AddFieldDialog", () => {
  let originalFetch: typeof global.fetch;
  let sent: { method: string; path: string; body: string }[];

  beforeEach(async () => {
    originalFetch = global.fetch;
    sent = [];
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });

  function mount(answer: (path: string) => Response) {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const path = new URL(request.url).pathname;
      sent.push({ method: request.method, path, body: request.method === "GET" ? "" : await request.clone().text() });
      return answer(path);
    }) as typeof fetch;
    const onProposed = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <AddFieldDialog project="hel" modelName="stations" source={STATIONS} type={TYPE} open onOpenChange={() => {}} onProposed={onProposed} />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    return onProposed;
  }

  const official = (path: string): Response | undefined => {
    if (path.endsWith("/tools/sdm-catalog")) {
      return respond({ subjects: [{ name: "Transportation", models: [{ id: "dataModel.Transportation/BikeHireDockingStation", name: TYPE }] }] });
    }
    if (path.endsWith("/tools/import-sdm")) return respond({ linkml: OFFICIAL });
    return undefined;
  };

  it("proposes the official attribute as a model change, its IRI and description with it", async () => {
    const onProposed = mount((path) => official(path) ?? respond({ metadata: { name: "chg-1" }, status: { phase: "Proposed" } }, 202));
    const offer = await screen.findByRole("button", { name: en.spaces.fields.proposeOfficial.replace("{name}", "availableBikeNumber") });
    await userEvent.click(offer);

    await waitFor(() => expect(onProposed).toHaveBeenCalledTimes(1));
    const put = sent.find((r) => r.method === "PUT")!;
    expect(put.path).toBe("/api/v1/projects/hel/datamodels/stations/source");
    const proposed = parseModel(put.body);
    expect(proposed.slots.find((s) => s.name === "availableBikeNumber")).toMatchObject({
      range: "integer",
      slot_uri: "https://smartdatamodels.org/dataModel.Transportation/availableBikeNumber",
      description: "Number of bikes available",
      upstream: "https://smartdatamodels.org/dataModel.Transportation/BikeHireDockingStation",
    });
    expect(proposed.classes.find((c) => c.name === TYPE)?.slots).toContain("availableBikeNumber");
  });

  it("says what to correct before anything is sent, and shows the Portal's refusal", async () => {
    mount((path) => official(path) ?? respond({ title: "Conflict", detail: "the model changed since it was read" }, 409));
    await screen.findByRole("button", { name: en.spaces.fields.proposeOfficial.replace("{name}", "status") });
    const dialog = screen.getByRole("dialog");

    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.fields.propose }));
    expect(within(dialog).getByLabelText(new RegExp(en.spaces.fields.name))).toHaveAccessibleDescription(
      expect.stringContaining(en.spaces.fields.problem.nameInvalid),
    );
    expect(sent.some((r) => r.method === "PUT")).toBe(false);

    await userEvent.type(within(dialog).getByLabelText(new RegExp(en.spaces.fields.name)), "capacity");
    await userEvent.selectOptions(within(dialog).getByLabelText(en.spaces.fields.type), "integer");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.fields.propose }));
    expect(await within(dialog).findByText("the model changed since it was read")).toBeInTheDocument();
  });

  function mountFormula(answer: (path: string, method: string) => Response, endpoints: unknown[]) {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const url = new URL(request.url);
      sent.push({ method: request.method, path: url.pathname + url.search, body: request.method === "GET" ? "" : await request.clone().text() });
      return official(url.pathname) ?? answer(url.pathname, request.method);
    }) as typeof fetch;
    const onProposed = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <AddFieldDialog
            project="hel"
            modelName="stations"
            source={STATIONS}
            type={TYPE}
            open
            onOpenChange={() => {}}
            onProposed={onProposed}
            space="bikes"
            endpoints={endpoints as never[]}
            orgDomain="hel.fi"
          />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    return onProposed;
  }

  async function fillFormula(expression: string) {
    const dialog = screen.getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(new RegExp(en.spaces.fields.name)), "title");
    await userEvent.selectOptions(within(dialog).getByLabelText(en.spaces.fields.type), "formula");
    await userEvent.type(within(dialog).getByLabelText(new RegExp(en.spaces.fields.formula.expression)), expression.replace(/[{[]/g, "$&$&"));
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.fields.propose }));
    return dialog;
  }

  const ENDPOINTS = [
    { apiVersion: "x", kind: "Endpoint", metadata: { name: "bikes-public" }, spec: { slug: "a", policyRef: "urn:ngsi-ld:Policy:hel.fi:bikes:public" } },
    { apiVersion: "x", kind: "Endpoint", metadata: { name: "bikes-all" }, spec: { slug: "b" } },
  ];

  it("proposes a formula field and then the class's pipeline and its Policies through an Endpoint bound to no single Policy", async () => {
    const onProposed = mountFormula((path, method) => {
      if (path.endsWith("/serviceaccounts/pipelines")) return respond({ metadata: { name: "pipelines" } });
      if (method === "PUT") return respond({ metadata: { name: "chg-model" }, status: { phase: "PendingApproval" } }, 202);
      if (path.endsWith("/import")) return respond({ metadata: { name: "chg-pipeline" }, status: { phase: "PendingApproval" } }, 202);
      return respond({}, 404);
    }, ENDPOINTS);
    await screen.findByText(en.spaces.fields.sdmTitle);
    await fillFormula("{name} + ' station'");

    await waitFor(() => expect(onProposed).toHaveBeenCalledTimes(2));
    expect(onProposed.mock.calls.map((call) => (call[0] as { metadata: { name: string } }).metadata.name)).toEqual(["chg-model", "chg-pipeline"]);
    const model = parseModel(sent.find((r) => r.method === "PUT")!.body);
    expect(model.slots.find((s) => s.name === "title")?.equals_expression).toBe("{name} + ' station'");
    const imports = sent.filter((r) => r.path.startsWith("/api/v1/projects/hel/import"));
    expect(imports.map((r) => r.path)).toEqual(["/api/v1/projects/hel/import?dryRun=All", "/api/v1/projects/hel/import"]);
    const body = JSON.parse(imports[1].body) as { conflictPolicy: string; manifests: { kind: string; metadata: { name: string }; spec: Record<string, unknown> }[] };
    expect(body.conflictPolicy).toBe("replace");
    expect(body.manifests.map((m) => `${m.kind}/${m.metadata.name}`)).toEqual([
      "Policy/formulas-bikehiredockingstation-read",
      "Policy/formulas-bikehiredockingstation-write",
      "Pipeline/formulas-bikehiredockingstation",
    ]);
    expect(body.manifests[2].spec).toMatchObject({ targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-all", source: { endpointRef: { name: "bikes-all" } } });
  });

  it("proposes nothing when the project has no pipelines account, and says so", async () => {
    const onProposed = mountFormula(
      (path) => (path.endsWith("/serviceaccounts/pipelines") ? respond({ title: "Not Found" }, 404) : respond({}, 500)),
      ENDPOINTS,
    );
    await screen.findByText(en.spaces.fields.sdmTitle);
    const dialog = await fillFormula("{name} + ' station'");
    expect(await within(dialog).findByText(en.spaces.fields.formula.noAccount)).toBeInTheDocument();
    expect(onProposed).not.toHaveBeenCalled();
    expect(sent.some((r) => r.method === "PUT" || r.path.includes("/projects/hel/import"))).toBe(false);
  });

  it("says why a formula cannot be computed before anything is sent", async () => {
    mountFormula(() => respond({}, 500), ENDPOINTS);
    await screen.findByText(en.spaces.fields.sdmTitle);
    const dialog = await fillFormula("{capacity} * 2");
    expect(within(dialog).getByLabelText(new RegExp(en.spaces.fields.formula.expression))).toHaveAccessibleDescription(
      expect.stringContaining("{capacity} is not a slot of this class"),
    );
    expect(sent.some((r) => r.path.includes("/serviceaccounts/"))).toBe(false);
  });

  it("still offers an own field when the catalogue cannot be reached", async () => {
    mount((path) => (path.includes("/tools/") ? respond({ title: "Bad Gateway" }, 502) : respond({}, 500)));
    expect(await screen.findByText(en.spaces.fields.sdmUnavailable)).toBeInTheDocument();
    expect(screen.getByLabelText(en.spaces.fields.type)).toBeEnabled();
  });
});
