/**
 * T-2766 (DM-61, DM-62, UI-85): every place that names a data model or an entity type links to
 * it, a type opening its class, and a name nobody may open stays text.
 */
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { jsonResponse, list, renderRoute } from "./pageHarness";
import { ModelLink, TypeLink, modelOfType } from "../src/pages/models/ModelLinks";
import { QueryAnswer } from "../src/pages/apps/QueryResultCard";
import { EndpointProposalCard } from "../src/pages/apps/EndpointProposalCard";
import { CatalogCards } from "../src/pages/apps/CatalogCards";
import type { Manifest } from "../src/api/manifest";

const manifest = (kind: string, name: string, spec: Record<string, unknown>, metadata: Record<string, unknown> = {}) =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind, metadata: { name, namespace: "helsinki", ...metadata }, spec }) as Manifest;

const AIR = `id: https://hel.fi/models/air
name: air
classes:
  AirQualityObserved:
    slots: [pm10]
  Station:
    slots: [name]
slots:
  pm10: { range: float }
  name: { range: string }
`;

const MODELS = [
  manifest("DataModel", "air", { contextSpaceRef: "ilma", linkml: AIR, classes: ["AirQualityObserved", "Station"] }),
  manifest("DataModel", "stations", { contextSpaceRef: "mobility", linkml: "./stations.linkml.yaml", classes: ["Station"] }),
];

const LISTS: Record<string, Manifest[]> = {
  datamodels: MODELS,
  spaces: [manifest("ContextSpace", "ilma", {})],
  endpoints: [manifest("Endpoint", "air-public", { contextSpaceRef: "ilma", slug: "airslug0000000000000000000000000", audience: "public" })],
  subscriptions: [
    manifest("Subscription", "air-alerts", {
      contextSpaceRef: "ilma",
      entities: [{ type: "AirQualityObserved" }, { idPattern: "urn:ngsi-ld:Station:.*" }],
      watchedAttributes: ["pm10"],
      notification: { endpoint: { uri: "https://hooks.hel.fi/air" } },
    }),
  ],
  pipelines: [manifest("Pipeline", "air-ingest", { output: { type: "AirQualityObserved", mode: "upsert" } })],
  projections: [
    manifest("ModelProjection", "air-open", { contextSpaceRef: "ilma", dataModelRef: "air", classes: [{ name: "AirQualityObserved", slots: ["pm10"] }] }),
  ],
};

function answer(path: string): Response | undefined {
  const plural = /^\/api\/v1\/projects\/helsinki\/([a-z]+)$/.exec(path)?.[1];
  if (plural && LISTS[plural]) {
    return jsonResponse(list(LISTS[plural]));
  }
  if (path === "/api/v1/projects/helsinki/spaces/ilma") {
    return jsonResponse(LISTS.spaces[0]);
  }
  if (path === "/api/v1/projects/helsinki/endpoints/air-public") {
    return jsonResponse(manifest("Endpoint", "air-public", { ...LISTS.endpoints[0].spec, projectionRef: { kind: "ModelProjection", name: "air-open" } }));
  }
  if (path === "/api/v1/tools/generate") {
    return jsonResponse({
      jsonSchema: {
        definitions: {
          AirQualityObserved: { type: "object", properties: { pm10: { type: "number", title: "pm10" } } },
          Station: { type: "object", properties: { name: { type: "string", title: "name" } } },
        },
      },
      example: { type: "Station", name: "Kallio" },
    });
  }
  return undefined;
}

/** A component inside a router, with the project's model list already read (or refused). */
function renderWithModels(node: ReactNode, models: Manifest[] | Error = MODELS) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  if (models instanceof Error) {
    client.setQueryDefaults(queryKeys.list("helsinki", "datamodels"), { queryFn: () => Promise.reject(models) });
  } else {
    client.setQueryData(queryKeys.list("helsinki", "datamodels"), list(models));
  }
  const rootRoute = createRootRoute({ component: () => <>{node}</> });
  const router = createRouter({ routeTree: rootRoute });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const href = (name: string) => screen.getByRole("link", { name }).getAttribute("href");

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the links", () => {
  it("find the model that carries a type, the space's own first", () => {
    expect(modelOfType(MODELS, "Station")?.metadata.name).toBe("air");
    expect(modelOfType(MODELS, "Station", "mobility")?.metadata.name).toBe("stations");
    expect(modelOfType(MODELS, "Station", "elsewhere")?.metadata.name).toBe("air");
    expect(modelOfType(MODELS, "Nobody")).toBeUndefined();
    expect(modelOfType(undefined, "Station")).toBeUndefined();
  });

  it("link a type to its class and a model to its page", async () => {
    renderWithModels(
      <>
        <TypeLink project="helsinki" type="Station" space="mobility" />
        <ModelLink project="helsinki" name="air" />
      </>,
    );
    expect(await screen.findByRole("link", { name: "Station" })).toHaveAttribute("href", "/projects/helsinki/models/stations?class=Station");
    expect(href("air")).toBe("/projects/helsinki/models/air");
  });

  it("leave a name as text when no model carries it or the models cannot be read", async () => {
    renderWithModels(
      <>
        <TypeLink project="helsinki" type="Parking" />
        <ModelLink project="helsinki" name="gone" />
      </>,
    );
    expect(await screen.findByText("Parking")).toBeInTheDocument();
    expect(screen.getByText("gone")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("offer nothing to open to a person who may not read the models", async () => {
    renderWithModels(<TypeLink project="helsinki" type="Station" />, new Error("403 Forbidden"));
    expect(await screen.findByText("Station")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });
});

describe("the places that name a model or a type", () => {
  it("the space's entity types", async () => {
    await renderRoute({ path: "/projects/helsinki/spaces/ilma?tab=model", answer });
    const types = await screen.findByRole("table", { name: en.spaces.inside.types });
    expect(await within(types).findByRole("link", { name: "Station" })).toHaveAttribute("href", "/projects/helsinki/models/air?class=Station");
  });

  it("a pipeline's target type", async () => {
    await renderRoute({ path: "/projects/helsinki/pipelines", answer });
    // Past the harness's 5 s on a CI runner under coverage (ci-full 39901479856).
    const link = await screen.findByRole("link", { name: "AirQualityObserved" }, { timeout: 10_000 });
    expect(link).toHaveAttribute("href", "/projects/helsinki/models/air?class=AirQualityObserved");
    expect(link.parentElement).toHaveTextContent(`${en.pipelines.writesType} AirQualityObserved`);
  });

  it("a subscription's type, its id pattern left as text", async () => {
    await renderRoute({ path: "/projects/helsinki/subscriptions", answer });
    const link = await screen.findByRole("link", { name: "AirQualityObserved" });
    expect(link).toHaveAttribute("href", "/projects/helsinki/models/air?class=AirQualityObserved");
    expect(link.parentElement).toHaveTextContent("AirQualityObserved, urn:ngsi-ld:Station:.*, pm10");
  });

  it("the Explore type", async () => {
    await renderRoute({ path: "/projects/helsinki/explore?space=ilma&endpoint=air-public&type=Station", answer });
    expect(await screen.findByRole("link", { name: "Station" })).toHaveAttribute("href", "/projects/helsinki/models/air?class=Station");
  });

  it("a model's classes in the list, and a class the model page opens on", async () => {
    await renderRoute({ path: "/projects/helsinki/models", answer });
    const table = await screen.findByRole("table", { name: en.models.page.listCaption });
    expect((await within(table).findAllByRole("link", { name: "Station" }))[0]).toHaveAttribute("href", "/projects/helsinki/models/air?class=Station");
  });

  it("the model page opens the form on the class a link named", async () => {
    await renderRoute({ path: "/projects/helsinki/models/air?class=Station", answer });
    expect(await screen.findByRole("tab", { name: en.models.page.view.form, selected: true })).toBeInTheDocument();
    expect(await screen.findByLabelText(en.models.page.formClass)).toHaveValue("Station");
  });

  it("the assistant's schema answers, and plain text without a project", async () => {
    const index = { kind: "schemaIndex" as const, models: [{ name: "air", semver: "1.2.0", types: ["AirQualityObserved", "Station"] }], formats: ["linkml"], recommended: "linkml" };
    const { unmount } = renderWithModels(<QueryAnswer view={index} project="helsinki" />);
    expect(await screen.findByRole("link", { name: "air" })).toHaveAttribute("href", "/projects/helsinki/models/air");
    expect(href("Station")).toBe("/projects/helsinki/models/air?class=Station");
    expect(screen.getAllByRole("listitem")[0].textContent).toContain(
      `AirQualityObserved, Station, ${en.assistant.query.fromModel} air 1.2.0`,
    );
    unmount();
    renderWithModels(<QueryAnswer view={index} />);
    expect(await screen.findByText(/AirQualityObserved, Station/)).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("the assistant's endpoint proposal", async () => {
    renderWithModels(
      <EndpointProposalCard
        project="helsinki"
        proposal={{
          endpoint: { metadata: { name: "air-open" }, spec: { contextSpaceRef: "ilma", audience: "public" } },
          policies: [{ spec: { information: [{ entities: [{ type: "AirQualityObserved" }] }] } }],
          prefill: {},
          lane: "green",
          slug: "airopen000000000000000000000000a",
        }}
      />,
    );
    expect(await screen.findByRole("link", { name: "AirQualityObserved" })).toHaveAttribute(
      "href",
      "/projects/helsinki/models/air?class=AirQualityObserved",
    );
  });

  it("a data model the catalog found opens its own page", async () => {
    renderWithModels(
      <CatalogCards
        project="helsinki"
        now={Date.parse("2026-09-25T08:00:00Z")}
        items={[{ kind: "DataModel", name: "air", space: "ilma", owner: "helsinki", title: "Air quality", matchReason: ["title"], access: { verdict: "allowed", reason: "" }, freshness: null }]}
      />,
    );
    expect(await screen.findByRole("link", { name: "Air quality" })).toHaveAttribute("href", "/projects/helsinki/models/air");
  });

  it("a dashboard's grid names its type as a link (guard: no bare type as the title)", () => {
    // The guard T-2766 asks for: the pages that have the data to link must not print it bare.
    const source = (file: string) => readFileSync(resolve(__dirname, "../src", file), "utf8");
    expect(source("routes/DashboardsPage.tsx")).toContain("title={<TypeLink project={project} type={widget.entityType} />}");
    for (const [file, bare] of [
      ["routes/DashboardsPage.tsx", "title={widget.entityType}"],
      ["pages/endpoints/EndpointPage.tsx", "<Badge mono>{klass}</Badge>"],
      ["pages/spaces/SpaceInside.tsx", '<TableCell className="font-mono">{type}</TableCell>'],
      ["pages/spaces/SpaceInside.tsx", '<span className="font-mono">{model.metadata.name}</span>'],
      ["pages/apps/EndpointProposalCard.tsx", '[...new Set(types)].join(", ")'],
      ["pages/models/ModelsList.tsx", 'row.classes.join(", ")'],
    ] as const) {
      expect(source(file), `${file} prints a name bare: ${bare}`).not.toContain(bare);
    }
  });
});
