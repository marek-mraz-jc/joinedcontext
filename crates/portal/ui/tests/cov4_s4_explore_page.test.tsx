// T-7706: the explorer's remaining paths: failed lists with Retry, a project out of reach, the first pick of a space, the attributes asked for, a relationship opened and the way back, and the map of a located type.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { InRouter } from "./pageHarness";
import { ExplorePage } from "../src/pages/explore/ExplorePage";
import type { Bbox, MapLayer } from "../src/components/dashboards/MapLibreView";

const drawn = vi.hoisted(() => ({ layers: [] as MapLayer[] }));
vi.mock("../src/components/dashboards/MapLibreView", async (original) => ({
  ...(await original<typeof import("../src/components/dashboards/MapLibreView")>()),
  MapLibreView: ({ layers, label }: { layers: MapLayer[]; label: string; onMoveEnd?: (bbox: Bbox) => void }) => {
    drawn.layers = layers;
    return <div role="application" aria-label={label} />;
  },
}));

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const ENDPOINTS = list([
  {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name: "bikes-ops", namespace: "helsinki" },
    spec: { contextSpaceRef: "helsinki", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld"] },
  },
]);
const SPACES = list([{ apiVersion: API, kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { dataModelRef: "bikes" } }]);
const MODEL = [
  "id: https://hel.fi/models/bikes",
  "name: bikes",
  "classes:",
  "  Station:",
  "    slots: [id, name, operator, location, entrance]",
  "  Operator:",
  "    slots: [id, name, stations]",
  "slots:",
  "  id: {}",
  "  name: {}",
  "  location: { annotations: { ngsi_ld_kind: GeoProperty } }",
  "  entrance: { annotations: { ngsi_ld_kind: GeoProperty } }",
  "  operator:",
  "    range: Operator",
  "    inverse: stations",
  "    annotations: { ngsi_ld_kind: Relationship }",
  "  stations:",
  "    range: Station",
  "    multivalued: true",
  "    inverse: operator",
  "    annotations: { ngsi_ld_kind: Relationship }",
  "",
].join("\n");
const MODELS = list([
  { apiVersion: API, kind: "DataModel", metadata: { name: "bikes", namespace: "helsinki" }, spec: { classes: ["Station", "Operator"], linkml: MODEL } },
]);
const STATION_ID = "urn:ngsi-ld:Station:hel.fi:helsinki:001";
const OPERATOR_ID = "urn:ngsi-ld:Operator:hel.fi:helsinki:op1";
const STATION = {
  id: STATION_ID,
  type: "Station",
  name: { type: "Property", value: "Kamppi" },
  operator: { type: "Relationship", object: OPERATOR_ID },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
};
const OPERATOR = { id: OPERATOR_ID, type: "Operator", name: { type: "Property", value: "City Bikes Oy" } };

interface Options {
  spaces?: number;
  endpoints?: number;
  models?: number;
  initialEndpoint?: string;
  initialSpace?: string;
  initialType?: string;
  initialView?: "map";
}

function mount(options: Options = {}) {
  const asked: URL[] = [];
  const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
    new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", ...headers } });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown) => {
      const url = new URL(typeof input === "string" ? input : (input as Request).url, window.location.origin);
      asked.push(url);
      const failing = (status: number | undefined) => (status ? json({ title: "Unavailable", status, detail: "The service is unavailable" }, status) : undefined);
      if (url.pathname.endsWith("/spaces") && options.spaces === 404) {
        return Promise.resolve(json({ title: "Not Found", status: 404, detail: "project 'helsinki' not found" }, 404));
      }
      if (url.pathname.endsWith("/spaces")) return Promise.resolve(failing(options.spaces) ?? json(SPACES));
      if (url.pathname.endsWith("/endpoints")) return Promise.resolve(failing(options.endpoints) ?? json(ENDPOINTS));
      if (url.pathname.endsWith("/datamodels")) return Promise.resolve(failing(options.models) ?? json(MODELS));
      if (url.pathname.includes("/access/attributes") && url.searchParams.has("attribute")) {
        return Promise.resolve(json({ policy: "bikes-readers", policyTitle: "Bike readers" }));
      }
      if (url.pathname.includes("/access/attributes")) {
        const attributes = ["id", "name", "operator", "location", "entrance"].map((name) => ({ name, read: true, write: false, delete: false, reason: name === "name" ? "prohibited" : "grant" }));
        return Promise.resolve(json({ type: url.searchParams.get("type"), attributes }));
      }
      if (url.pathname.includes("/access/check")) return Promise.resolve(json({ decision: true }));
      if (url.pathname.includes("/access")) return Promise.resolve(json({ permissions: [{ actions: ["queryEntity", "retrieveEntity"] }], prohibitions: [] }));
      const byId = [STATION, OPERATOR].find((entity) => url.pathname.endsWith(`/entities/${encodeURIComponent(entity.id)}`) || url.pathname.endsWith(`/entities/${entity.id}`));
      if (byId) return Promise.resolve(json(byId));
      if (url.pathname.includes("/entities")) {
        const types = (url.searchParams.get("type") ?? "").split(",");
        const rows = [STATION, OPERATOR].filter((entity) => types.includes(entity.type));
        return Promise.resolve(json(rows, 200, { "NGSILD-Results-Count": String(rows.length) }));
      }
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  window.history.replaceState(null, "", "/");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage
            project="helsinki"
            initialSpace={options.initialSpace}
            initialEndpoint={options.initialEndpoint}
            initialType={options.initialType}
            initialView={options.initialView}
          />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return asked;
}

const reads = (asked: URL[], suffix: string) => asked.filter((url) => url.pathname.endsWith(suffix)).length;

beforeEach(async () => {
  drawn.layers = [];
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer when a list does not come", () => {
  it("says why the spaces list failed and reads it again on Retry", async () => {
    const asked = mount({ spaces: 500, initialSpace: "helsinki", initialEndpoint: "bikes-ops" });
    expect(await screen.findByText(/The service is unavailable/)).toBeInTheDocument();
    const before = reads(asked, "/spaces");
    await userEvent.click(screen.getByRole("button", { name: en.form.listRetry }));
    await waitFor(() => expect(reads(asked, "/spaces")).toBeGreaterThan(before));
  });

  it("says why the endpoints list failed and reads it again on Retry", async () => {
    const asked = mount({ endpoints: 503, initialSpace: "helsinki" });
    expect(await screen.findByText(/The service is unavailable/)).toBeInTheDocument();
    const before = reads(asked, "/endpoints");
    await userEvent.click(screen.getByRole("button", { name: en.form.listRetry }));
    await waitFor(() => expect(reads(asked, "/endpoints")).toBeGreaterThan(before));
  });

  it("says why the models list failed and reads it again on Retry", async () => {
    const asked = mount({ models: 500, initialSpace: "helsinki", initialEndpoint: "bikes-ops" });
    await screen.findByText(/The service is unavailable/);
    const before = reads(asked, "/datamodels");
    await userEvent.click(screen.getByRole("button", { name: en.form.listRetry }));
    await waitFor(() => expect(reads(asked, "/datamodels")).toBeGreaterThan(before));
  });

  it("shows the project as hidden when the API says it does not exist for this person", async () => {
    mount({ spaces: 404 });
    expect(await screen.findByRole("heading", { name: en.explore.title })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByLabelText(en.explore.space)).toBeNull());
  });

  it("offers the project's spaces as the first pick when a linked endpoint is in none", async () => {
    // The endpoints list did not come, so the linked endpoint cannot be placed in any space.
    mount({ initialEndpoint: "ghost", endpoints: 503 });
    expect(await screen.findByText(en.explore.noSpace)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "helsinki" }));
    await waitFor(() => expect(screen.queryByText(en.explore.noSpace)).toBeNull());
    expect(screen.getByLabelText(en.explore.space)).toHaveValue("helsinki");
  });
});

describe("the explorer's attributes, relationships and map", () => {
  it("asks the endpoint for the attributes the person ticks", async () => {
    const asked = mount({ initialSpace: "helsinki", initialEndpoint: "bikes-ops", initialType: "Station" });
    await userEvent.click(await screen.findByRole("checkbox", { name: "name" }));
    await waitFor(() =>
      expect(asked.some((url) => url.pathname.includes("/entities") && (url.searchParams.get("attrs") ?? "").includes("name"))).toBe(true),
    );
  });

  it("opens the entity a relationship points to, and goes back to the one it came from", async () => {
    mount({ initialSpace: "helsinki", initialEndpoint: "bikes-ops", initialType: "Station" });
    await userEvent.click(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: Kamppi` }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(await within(dialog).findByRole("button", { name: /City Bikes Oy/ }));
    const back = await screen.findByRole("button", { name: en.explore.backToRow });
    await userEvent.click(back);
    await waitFor(() => expect(screen.queryByRole("button", { name: en.explore.backToRow })).toBeNull());
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("explains an attribute's access from the policy that decides it", async () => {
    const asked = mount({ initialSpace: "helsinki", initialEndpoint: "bikes-ops", initialType: "Station" });
    await userEvent.click(await screen.findByRole("button", { name: "Your access: name" }));
    await waitFor(() => expect(asked.some((url) => url.searchParams.get("attribute") === "name")).toBe(true));
    const request = asked.find((url) => url.searchParams.get("attribute") === "name");
    expect(request?.pathname).toBe("/api/v1/projects/helsinki/endpoints/bikes-ops/access/attributes");
    expect(request?.searchParams.get("attribute")).toBe("name");
    expect(request?.searchParams.get("type")).toBe("Station");
    expect(await screen.findByText(/Bike readers/)).toBeInTheDocument();
  });

  it("draws a located type on the map with the model's place and plain attributes", async () => {
    mount({ initialSpace: "helsinki", initialEndpoint: "bikes-ops", initialType: "Station", initialView: "map" });
    expect(await screen.findByRole("application")).toBeInTheDocument();
    await waitFor(() => expect(drawn.layers.length).toBeGreaterThan(0));
  });
});
