/**
 * T-2872: an edit form opens with every stored value in its field, a reference the manifest writes
 * typed (`dataModelRef: { kind: DataModel, name: praha-mesto }`, as the seeds do) included.
 *
 * The Prague space opened with Data model empty: the form held the object, the picker reads a name,
 * and a Save proposed a space without its model. `namesOfRefs` holds every picker's reference by
 * its name in the shared form; each kind's own mapper is checked here for the fields it fills.
 * Organization settings hold no reference; their pre-fill is organization_page.test.tsx's.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { namesOfRefs } from "../src/schemas/pickers";
import { contextSpaceSchema, endpointSchema } from "../src/schemas/kinds";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { EditResourceDialog } from "../src/components/EditResourceDialog";
import { fromEnvelope, toEnvelope } from "../src/routes/SpacesPage";
import { toForm as endpointForm } from "../src/routes/EndpointsPage";
import { fromManifest as pipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { fromAppEnvelope } from "../src/pages/apps/appForm";
import { fromPolicyEnvelope } from "../src/routes/PoliciesPage";
import type { Manifest } from "../src/api/manifest";

// The Change the dialog shows on success links to its approval through the router.
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to }: { children: ReactNode; to: string }) => <a href={to}>{children}</a>,
}));

const PRAHA = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "praha-mesto", namespace: "praha", title: { cs: "Praha — městská data", en: "Prague city data" } },
  spec: {
    urnSegment: "praha-mesto",
    isSandbox: false,
    dataModelRef: { kind: "DataModel", name: "praha-mesto" },
    defaultLocale: "cs",
    missingUnitCode: "refuse",
  },
  status: { phase: "Ready" },
};

const MODELS = [
  { name: "praha-mesto", project: "praha", space: "praha-mesto", version: "1.0.0", lifecycle: "published", classes: ["BikeHireDockingStation"] },
  { name: "praha-doprava", project: "praha", space: "praha-mesto", version: "0.2.0", lifecycle: "draft", classes: ["Road"] },
];

let sent: { method: string; path: string; body: unknown }[] = [];

function stubFetch() {
  sent = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const url = new URL(request.url);
      const body = request.method === "GET" ? undefined : await request.clone().json();
      sent.push({ method: request.method, path: url.pathname + url.search, body });
      if (url.pathname === "/api/v1/organization/datamodels") {
        return Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: MODELS, smartDataModels: [] });
      }
      if (url.pathname === "/api/v1/projects/praha/spaces/praha-mesto" && request.method === "GET") {
        return Response.json(PRAHA);
      }
      if (url.pathname === "/api/v1/projects/helsinki/spaces") {
        return Response.json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: ["air", "helsinki"].map((name) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name }, spec: {} })),
        });
      }
      if (request.method === "PUT" && url.searchParams.get("dryRun") === "All") {
        return Response.json({ valid: true, verdict: { ok: true, findings: [], checkedAt: "", inputDigest: "sha256:0" } });
      }
      if (request.method === "PUT") {
        return Response.json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-00000001", namespace: "praha" },
          status: { lane: "yellow", phase: "PendingApproval" },
        });
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

/** What a select shows: an enum select holds the option's index, the person reads its words. */
const shownOption = (id: string) => (document.getElementById(id) as HTMLSelectElement | null)?.selectedOptions[0]?.textContent;

function wrap(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  stubFetch();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("namesOfRefs holds a typed reference by the name its picker reads", () => {
  it("names a typed reference at a field, a nested path and inside lists, and leaves the rest as it was", () => {
    const space = { name: "praha-mesto", dataModelRef: { kind: "DataModel", name: "praha-mesto" } };
    expect(namesOfRefs("ContextSpace", space)).toEqual({ name: "praha-mesto", dataModelRef: "praha-mesto" });
    // The input is the page's state, so it is never written to.
    expect(space.dataModelRef).toEqual({ kind: "DataModel", name: "praha-mesto" });

    expect(
      namesOfRefs("Endpoint", {
        contextSpaceRef: { kind: "ContextSpace", name: "helsinki" },
        catalog: { pipelineRef: { kind: "Pipeline", name: "hsl-hfp" }, license: "CC-BY-4.0" },
      }),
    ).toEqual({ contextSpaceRef: "helsinki", catalog: { pipelineRef: "hsl-hfp", license: "CC-BY-4.0" } });

    expect(
      namesOfRefs("Dashboard", {
        pages: [{ widgets: [{ endpointRef: { kind: "Endpoint", name: "air" } }, { endpointRef: "bikes" }, {}] }, {}],
      }),
    ).toEqual({ pages: [{ widgets: [{ endpointRef: "air" }, { endpointRef: "bikes" }, {}] }, {}] });
  });

  it("keeps what it cannot name: a bare name, an object without one, a kind with no pickers, no data", () => {
    const odd = { dataModelRef: { kind: "DataModel" } };
    expect(namesOfRefs("ContextSpace", odd)).toBe(odd);
    const plain = { dataModelRef: "praha-mesto" };
    expect(namesOfRefs("ContextSpace", plain)).toBe(plain);
    const unknown = { dataModelRef: { name: "x" } };
    expect(namesOfRefs("Organization", unknown)).toBe(unknown);
    expect(namesOfRefs(undefined, unknown)).toBe(unknown);
    expect(namesOfRefs("ContextSpace", undefined)).toBeUndefined();
    expect(namesOfRefs("Dashboard", { pages: "not a list" })).toEqual({ pages: "not a list" });
  });
});

describe("the Prague space edit (the owner's report)", () => {
  function openEdit() {
    const t = i18n.t.bind(i18n);
    return wrap(
      <EditResourceDialog
        target={{ project: "praha", kind: "ContextSpace", plural: "spaces", name: "praha-mesto" }}
        open
        onOpenChange={() => {}}
        form={{
          schema: contextSpaceSchema(t),
          fromManifest: (manifest) => fromEnvelope(manifest, "en") as unknown as Record<string, unknown>,
          toManifest: (edited, stored) => toEnvelope("praha", edited as never, stored, "en"),
        }}
      />,
    );
  }

  it("opens with every stored value in its field, the typed data model selected", async () => {
    openEdit();
    const model = await screen.findByRole("combobox", { name: new RegExp(en.spaces.field.dataModel) });
    await waitFor(() => expect((model as HTMLInputElement).value).toBe("praha-mesto"));
    expect(document.getElementById("root_name")).toHaveValue("praha-mesto");
    expect(document.getElementById("root_title")).toHaveValue("Prague city data");
    expect(shownOption("root_defaultLocale")).toBe(en.choice.language.cs);
    // The space's own answer to a missing unit, not the default, and nothing about the model.
    expect(shownOption("root_missingUnitCode")).toBe(en.choice.missingUnitCode.refuse);
  });

  it("an untouched Propose sends the model as the manifest wrote it", async () => {
    const user = userEvent.setup();
    openEdit();
    const model = await screen.findByRole("combobox", { name: new RegExp(en.spaces.field.dataModel) });
    await waitFor(() => expect((model as HTMLInputElement).value).toBe("praha-mesto"));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent.filter((request) => request.method === "PUT")).toHaveLength(2));
    for (const request of sent.filter((each) => each.method === "PUT")) {
      expect(request.path.startsWith("/api/v1/projects/praha/spaces/praha-mesto")).toBe(true);
      expect((request.body as { spec: unknown }).spec).toEqual(PRAHA.spec);
    }
  });

  it("a model picked instead is written by its name, and a space with none gets none", () => {
    const form = fromEnvelope(PRAHA, "en");
    expect(form.dataModelRef).toBe("praha-mesto");
    expect(toEnvelope("praha", { ...form, dataModelRef: "praha-doprava" }, PRAHA).spec.dataModelRef).toBe("praha-doprava");
    const bare = { ...PRAHA, spec: { ...PRAHA.spec, dataModelRef: "praha-mesto" } };
    expect(toEnvelope("praha", fromEnvelope(bare), bare).spec.dataModelRef).toBe("praha-mesto");
    const none = { ...PRAHA, spec: { defaultLocale: "cs" } };
    const empty = fromEnvelope(none);
    expect(empty).not.toHaveProperty("dataModelRef");
    expect(toEnvelope("praha", empty, none).spec).toEqual({ defaultLocale: "cs" });
  });
});

describe("the other edit forms fill their references from a typed manifest", () => {
  it("an endpoint shows its space selected, and its catalogue and pipeline by name", async () => {
    const endpoint = endpointForm({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "hsl-vehicles", namespace: "helsinki", title: "HSL vehicles" },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: "helsinki" },
        slug: "hsl-vehicles",
        audience: "public",
        publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" }, organization: "hsl" } },
        catalog: { pipelineRef: { kind: "Pipeline", name: "hsl-hfp-vehicles" } },
      },
    } as Manifest);
    expect(endpoint).toMatchObject({
      name: "hsl-vehicles",
      title: "HSL vehicles",
      contextSpaceRef: "helsinki",
      slug: "hsl-vehicles",
      audience: "public",
      publish: { ckan: { instanceRef: "hel-fi", organization: "hsl" } },
      catalog: { pipelineRef: "hsl-hfp-vehicles" },
    });

    // And the form: a typed space a page passes straight through still opens selected.
    wrap(
      <SchemaForm
        kind="Endpoint"
        project="helsinki"
        schema={endpointSchema(i18n.t.bind(i18n), ["air", "helsinki"])}
        formData={{ ...endpoint, contextSpaceRef: { kind: "ContextSpace", name: "helsinki" } }}
        onSubmit={() => {}}
      />,
    );
    await waitFor(() => expect(shownOption("root_contextSpaceRef")).toBe("helsinki"));
    expect(document.getElementById("root_name")).toHaveValue("hsl-vehicles");
  });

  it("a pipeline names its data source and endpoint", () => {
    const pipeline = pipelineForm({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "hsl-hfp", namespace: "helsinki" },
      spec: {
        source: { dataSourceRef: { kind: "DataSource", name: "hsl-hfp" } },
        output: { type: "Vehicle", endpointRef: { kind: "Endpoint", name: "hsl-vehicles" } },
      },
    });
    expect(pipeline.name).toBe("hsl-hfp");
    expect(pipeline.source?.dataSourceRef).toBe("hsl-hfp");
    expect(pipeline.output).toMatchObject({ type: "Vehicle" });
  });

  it("an app names the space of each data need", () => {
    const app = fromAppEnvelope({
      metadata: { name: "bikes" },
      spec: {
        kind: "static",
        visibility: "public",
        source: { path: "apps/bikes" },
        dataNeeds: [{ contextSpaceRef: { kind: "ContextSpace", name: "praha-mesto" }, types: ["BikeHireDockingStation"] }],
      },
    });
    expect(app).toMatchObject({ name: "bikes", kind: "ui", visibility: "public", source: { from: "path", path: "apps/bikes" } });
    expect(app.dataNeeds).toEqual([expect.objectContaining({ contextSpaceRef: "praha-mesto", types: ["BikeHireDockingStation"] })]);
  });

  it("a policy names its space", () => {
    const policy = fromPolicyEnvelope({
      metadata: { name: "public-read", namespace: "praha" },
      spec: { contextSpaceRef: { kind: "ContextSpace", name: "praha-mesto" } },
    });
    expect(policy).toMatchObject({ name: "public-read", contextSpaceRef: "praha-mesto" });
  });
});
