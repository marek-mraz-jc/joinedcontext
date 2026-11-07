/**
 * T-2762 (DM-61, DM-62, UI-84): inside a Context Space the data model is one click away. The
 * space page summarises the model and links its own page (T-6090), and a space with no model
 * offers Create and Import prefilled for it.
 */
import { screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { jsonResponse, list, renderRoute } from "./pageHarness";
import type { Manifest } from "../src/api/manifest";

const manifest = (kind: string, name: string, spec: Record<string, unknown>, metadata: Record<string, unknown> = {}) =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind, metadata: { name, namespace: "helsinki", ...metadata }, spec }) as Manifest;

const AIR = `id: https://hel.fi/models/air
name: air
classes:
  AirQualityObserved:
    slots: [pm10, refStation]
  Station:
    slots: [name]
  Any:
    class_uri: linkml:Any
slots:
  pm10: { range: float }
  refStation: { range: Station }
  name: { range: string }
`;

const MODEL = manifest(
  "DataModel",
  "air",
  { contextSpaceRef: "ilma", linkml: AIR, version: "1.2.0", classes: ["AirQualityObserved", "Station"] },
  { title: "Air quality" },
);

function answer(models: Manifest[]) {
  return (path: string): Response | undefined => {
    if (path === "/api/v1/projects/helsinki/spaces/ilma") {
      return jsonResponse(manifest("ContextSpace", "ilma", {}));
    }
    if (path === "/api/v1/projects/helsinki/datamodels") {
      return jsonResponse(list(models));
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
  };
}

/** A viewer: may read everything, may propose nothing. */
const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };

async function section(): Promise<ReturnType<typeof within>> {
  const heading = await screen.findByRole("heading", { level: 2, name: en.spaces.inside.model });
  return within(heading.closest("section") as HTMLElement);
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the space's data model", () => {
  // T-6090 (owner walk): the tab embedded the whole modeler, a second home for one model. It now
  // summarises the model and sends the person to the model's own page for everything else.
  it("summarises the model and links its one page", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/ilma?tab=model", answer: answer([MODEL]) });
    const model = await section();
    expect(model.getByText("Air quality")).toBeInTheDocument();
    expect(model.getByText("v1.2.0")).toBeInTheDocument();
    expect(model.getByRole("link", { name: en.spaces.inside.modelOpen })).toHaveAttribute(
      "href",
      "/projects/helsinki/models/air",
    );
    expect(await model.findByText("2 classes")).toBeInTheDocument();
    const classes = model.getByRole("list", { name: en.spaces.inside.modelClassList });
    expect(within(classes).getByRole("link", { name: "Station" })).toHaveAttribute(
      "href",
      "/projects/helsinki/models/air?class=Station",
    );
    // The modeler lives on the model's page only: no diagram, form, YAML or Edit here.
    expect(model.queryByRole("group", { name: en.models.graph.title })).toBeNull();
    expect(model.queryByRole("tab", { name: en.models.page.view.yaml })).toBeNull();
    expect(model.queryByRole("link", { name: en.models.page.edit })).toBeNull();
    // The model's name in the entity types line opens the model too.
    expect(screen.getByRole("link", { name: "air" })).toHaveAttribute("href", "/projects/helsinki/models/air");
  });

  // T-6090 review: a model listing no `spec.classes` showed "3 classes" on the card above "the data
  // model defines no entity type". The types now come from its source then.
  it("takes the entity types from the model's source when the manifest lists none", async () => {
    const unlisted = manifest("DataModel", "air", { contextSpaceRef: "ilma", linkml: AIR, version: "1.2.0" });
    await renderRoute({ path: "/projects/helsinki/spaces/ilma?tab=model", answer: answer([unlisted]) });
    const model = await section();
    expect(await model.findByText("2 classes")).toBeInTheDocument();
    const types = await screen.findByRole("table", { name: en.spaces.inside.types });
    expect(within(types).getByText("AirQualityObserved")).toBeInTheDocument();
    expect(within(types).getByText("Station")).toBeInTheDocument();
    expect(screen.queryByText(en.spaces.inside.noTypes.replace("{model}", "air"))).toBeNull();
  });

  it("offers Create and Import for this space when it has no model", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/ilma?tab=model", answer: answer([]) });
    const model = await section();
    expect(await model.findByText(en.spaces.inside.modelEmpty)).toBeInTheDocument();
    expect(model.getByRole("link", { name: en.spaces.inside.modelCreate })).toHaveAttribute(
      "href",
      "/projects/helsinki/models?new=blank&space=ilma",
    );
    expect(model.getByRole("link", { name: en.spaces.inside.modelImport })).toHaveAttribute(
      "href",
      "/projects/helsinki/models?new=sdm&space=ilma",
    );
  });

  it("lets a viewer open the model to read it", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/ilma?tab=model", answer: answer([MODEL]), permissions: READER });
    const model = await section();
    expect(await model.findByRole("link", { name: en.spaces.inside.modelOpen })).toHaveAttribute(
      "href",
      "/projects/helsinki/models/air",
    );
  });

  it("shows a viewer of an empty space the ways as disabled", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/ilma?tab=model", answer: answer([]), permissions: READER });
    const model = await section();
    expect(await model.findByRole("button", { name: en.spaces.inside.modelCreate })).toHaveAttribute("aria-disabled", "true");
    expect(model.getByRole("button", { name: en.spaces.inside.modelImport })).toHaveAttribute("aria-disabled", "true");
  });
});
