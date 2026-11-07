/**
 * T-3920, PL-73: a route sends the records its condition holds for to its own steps and outputs.
 * The canvas hangs each route off the lane's tail with its condition on the wire, the studio
 * edits it in place, and the manifest carries `spec.routes` in the order the form holds them.
 */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { toEnvelope, toForm } from "../src/pages/pipelines/PipelineEditor";
import { addRoute, addStep, removeRouteNode, toFlow } from "../src/pages/pipelines/PipelineFlow";
import { connect, laneOf, missingOf } from "../src/pages/pipelines/pipelineGraph";
import { PipelineStudio } from "../src/pages/pipelines/PipelineStudio";

vi.mock("../src/branding", () => ({
  useBranding: () => ({ orgDomain: "hel.fi" }),
}));

const LANE = "urn:ngsi-ld:Endpoint:hel.fi:air:ep-air";
const ALERTS = "urn:ngsi-ld:Endpoint:hel.fi:alerts:ep-alerts";

const form: PipelineForm = {
  name: "air",
  class: "auto",
  source: { dataSourceRef: "sensors" },
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: LANE,
  output: { mode: "upsert" },
  routes: [
    {
      name: "hot",
      when: "this.t > 30",
      steps: [{ kind: "bloblang", bloblang: 'root = this.merge({"alert": true})' }],
      outputs: [{ targetEndpoint: ALERTS, mode: "upsert" }],
    },
  ],
};

describe("routes on the canvas", () => {
  it("hangs a route off the lane's tail, the condition on its wire, its steps before its outputs", () => {
    const { nodes, edges } = toFlow(form);
    expect(nodes.map((node) => node.id)).toEqual(["source", "compute", "output", "route-0-step-0", "route-0-output-0"]);
    expect(edges).toEqual([
      { from: "source", to: "compute" },
      { from: "compute", to: "output" },
      { from: "compute", to: "route-0-step-0", route: 0 },
      { from: "route-0-step-0", to: "route-0-output-0" },
    ]);
    expect(laneOf(form)).toEqual(["compute"]);
  });

  it("wires a route without steps straight to its outputs, from every source when the lane is empty", () => {
    const bare: PipelineForm = { ...form, compute: undefined, moreSources: [{ dataSourceRef: "more" }], routes: [{ ...form.routes![0], steps: undefined }] };
    expect(toFlow(bare).edges.filter((edge) => edge.route === 0)).toEqual([
      { from: "source", to: "route-0-output-0", route: 0 },
      { from: "source-0", to: "route-0-output-0", route: 0 },
    ]);
  });

  it("adds a route with a free name and one output, and a step into it from its output", () => {
    const added = addRoute(form);
    expect(added.id).toBe("route-1-output-0");
    expect(added.form.routes?.[1]).toEqual({ name: "route-1", when: "", outputs: [{ mode: "upsert" }] });
    expect(addRoute({ ...form, routes: [{ name: "route-1" }] }).form.routes?.[1].name).toBe("route-2");
    const stepped = addStep(added.form, "route-1-output-0", "log");
    expect(stepped?.id).toBe("route-1-step-0");
    expect(stepped?.form.routes?.[1].steps).toEqual([{ processor: { log: {} } }]);
    expect(stepped?.form.processors).toBeUndefined();
  });

  it("removes a route's step, its outputs but the last, and the route with its last output", () => {
    const two: PipelineForm = { ...form, routes: [{ ...form.routes![0], outputs: [{ targetEndpoint: ALERTS }, { mode: "upsert" }] }] };
    expect(removeRouteNode(two, "route-0-step-0").routes?.[0].steps).toEqual([]);
    expect(removeRouteNode(two, "route-0-output-1").routes?.[0].outputs).toEqual([{ targetEndpoint: ALERTS }]);
    expect(removeRouteNode(form, "route-0-output-0").routes).toEqual([]);
  });

  it("keeps the wires a route already has and refuses any other into or out of it", () => {
    expect(connect(form, "compute", "route-0-step-0")).toEqual({ unchanged: true });
    expect(connect(form, "source", "route-0-step-0")).toEqual({ refused: "route" });
    expect(connect(form, "route-0-step-0", "compute")).toEqual({ refused: "route" });
    // Nothing comes out of a route's output, and nothing goes into a source from a route.
    expect(connect(form, "route-0-output-0", "compute")).toEqual({ refused: "outOfOutput" });
    expect(connect(form, "route-0-step-0", "source")).toEqual({ refused: "intoSource" });
  });

  it("says what a route lacks: its condition first, then an Endpoint or a mapping", () => {
    const empty: PipelineForm = { ...form, routes: [{ name: "r", when: " ", steps: [{ kind: "bloblang", bloblang: "" }], outputs: [{}] }] };
    expect(missingOf(empty, "route-0-output-0")).toBe("condition");
    const named: PipelineForm = { ...empty, routes: [{ ...empty.routes![0], when: "this.t > 1" }] };
    expect(missingOf(named, "route-0-output-0")).toBe("output");
    expect(missingOf(named, "route-0-step-0")).toBe("mapping");
    expect(missingOf(form, "route-0-output-0")).toBeUndefined();
    expect(missingOf(form, "route-0-step-0")).toBeUndefined();
  });

  it("writes the routes into the manifest, reads them back, and an emptied list removes them", () => {
    const manifest = toEnvelope("helsinki", form);
    expect(manifest.apiVersion).toBe("joinedcontext.com/v1alpha2");
    expect((manifest.spec as { routes: unknown }).routes).toEqual(form.routes);
    expect(toForm(manifest).routes).toEqual(form.routes);
    const removed = toEnvelope("helsinki", { ...form, routes: [] }, manifest);
    expect((removed.spec as { routes?: unknown }).routes).toBeUndefined();
    // A form that carries no routes keeps the edited manifest's.
    const kept = toEnvelope("helsinki", { ...form, routes: undefined }, manifest);
    expect((kept.spec as { routes: unknown }).routes).toEqual(form.routes);
  });
});

describe("a route edited in the studio", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify({ items: [] }), { headers: { "Content-Type": "application/json" } }))),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const endpoints: Manifest[] = [
    { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "ep-air", namespace: "helsinki" }, spec: { contextSpaceRef: "air" } },
    { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "ep-alerts", namespace: "helsinki" }, spec: { contextSpaceRef: "alerts" } },
  ];

  function Host({ seen }: { seen: (form: PipelineForm) => void }) {
    const [draft, setDraft] = useState<PipelineForm>({ ...form, routes: undefined });
    return (
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PipelineStudio
            project="helsinki"
            draft={draft}
            onChange={(next) => {
              seen(next);
              setDraft(next);
            }}
            dataSources={[]}
            endpoints={endpoints}
            toManifest={vi.fn()}
          />
        </I18nextProvider>
      </QueryClientProvider>
    );
  }

  it("adds a route, names its condition and Endpoint, shows the condition on the wire, and removes it", async () => {
    const seen = vi.fn();
    render(<Host seen={seen} />);
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.flow.addRoute }));
    expect(screen.getByTestId("flow-node-route-0-output-0")).toBeInTheDocument();
    expect(screen.getByTestId("flow-route-when-0")).toHaveTextContent(en.pipelines.flow.routeNoCondition);
    expect(screen.getByTestId("flow-node-state-route-0-output-0")).toHaveTextContent(en.pipelines.flow.state.missing.condition);

    await userEvent.type(screen.getByTestId("flow-route-when"), "this.t > 30");
    await userEvent.selectOptions(screen.getByLabelText(new RegExp(`^${en.pipelines.flow.outputWrites}`)), ALERTS);
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({ routes: [{ name: "route-1", when: "this.t > 30", outputs: [{ targetEndpoint: ALERTS, mode: "upsert" }] }] }),
    );
    expect(screen.getByTestId("flow-route-when-0")).toHaveTextContent("this.t > 30");

    await userEvent.click(screen.getByTestId("flow-route-remove"));
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ routes: [] }));
    expect(screen.queryByTestId("flow-node-route-0-output-0")).not.toBeInTheDocument();
  });

  it("removes a route with Delete on its only output", async () => {
    const seen = vi.fn();
    render(<Host seen={seen} />);
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.flow.addRoute }));
    screen.getByTestId("flow-node-route-0-output-0").focus();
    await userEvent.keyboard("{Delete}");
    expect(screen.queryByTestId("flow-node-route-0-output-0")).not.toBeInTheDocument();
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ routes: [] }));
  });
});
