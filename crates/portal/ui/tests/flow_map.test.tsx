/**
 * T-4862: the data flow map of a project. The layout puts every path left to right (broker,
 * source, pipeline, endpoint, space, and derived pipelines further right), a missing grant is red
 * with its fix, a node opens its detail and actions, and "Send to a space" opens the pipeline
 * editor on the source and the chosen target. The phone list holds the same nodes.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { FlowMapView, PipelineActions, SendToSpace } from "../src/pages/pipelines/FlowMapView";
import { columns, groups, layout, missingGrants, targets, targetUrn } from "../src/pages/pipelines/flowMap";
import type { FlowMap, FlowNode } from "../src/pages/pipelines/flowMap";
import { findFormPage } from "./formPage";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";
import { json, list, renderPage } from "./page_contract";

const PATH = "/projects/helsinki/pipelines?view=map";

const node = (kind: FlowNode["kind"], name: string, over: Partial<FlowNode> = {}): FlowNode => ({
  id: `helsinki/${kind}/${name}`,
  kind,
  name,
  project: "helsinki",
  opaque: false,
  ...over,
});

const MAP: FlowMap = {
  project: "helsinki",
  nodes: [
    node("MqttBroker", "hsl"),
    node("DataSource", "hfp", { sourceType: "mqtt", hosts: ["mqtt.hsl.fi:8883"], topics: ["/hfp/v2/#"] }),
    node("Pipeline", "hfp-ingest", {
      role: "ingest",
      phase: "Live",
      enabled: true,
      lastRun: { at: "2026-10-21T04:00:00Z", sent: 1200, rejected: 3, failed: 0, perMinute: 20 },
    }),
    node("Endpoint", "ep-vehicles", { space: "vehicles", slug: "vehicles" }),
    node("ContextSpace", "vehicles"),
    node("Pipeline", "stop-delays", { role: "derived", phase: "Live", enabled: false }),
    node("Endpoint", "ep-stops", { space: "stops" }),
    node("ContextSpace", "stops"),
  ],
  edges: [
    { from: "helsinki/MqttBroker/hsl", to: "helsinki/DataSource/hfp", relation: "connects" },
    { from: "helsinki/DataSource/hfp", to: "helsinki/Pipeline/hfp-ingest", relation: "feeds" },
    {
      from: "helsinki/Pipeline/hfp-ingest",
      to: "helsinki/Endpoint/ep-vehicles",
      relation: "writes",
      grant: { state: "granted", endpoint: "ep-vehicles", operation: "upsertBatch", policy: "vehicles-pipelines" },
    },
    { from: "helsinki/Endpoint/ep-vehicles", to: "helsinki/ContextSpace/vehicles", relation: "stores" },
    { from: "helsinki/ContextSpace/vehicles", to: "helsinki/Endpoint/ep-vehicles", relation: "exposes" },
    {
      from: "helsinki/Endpoint/ep-vehicles",
      to: "helsinki/Pipeline/stop-delays",
      relation: "reads",
      grant: { state: "granted", endpoint: "ep-vehicles", operation: "queryEntity", policy: "vehicles-pipelines" },
    },
    {
      from: "helsinki/Pipeline/stop-delays",
      to: "helsinki/Endpoint/ep-stops",
      relation: "writes",
      grant: {
        state: "missing",
        endpoint: "ep-stops",
        operation: "upsertBatch",
        fix: "add a Policy on space stops that grants the ServiceAccount pipelines upsertBatch",
      },
    },
    { from: "helsinki/Endpoint/ep-stops", to: "helsinki/ContextSpace/stops", relation: "stores" },
  ],
};

const answering = (map: FlowMap) => (path: string) =>
  path.endsWith("/flow-map") ? jsonResponse(map) : undefined;

describe("the flow map layout", () => {
  it("puts_each_path_left_to_right_and_a_derived_pipeline_right_of_the_space_it_reads", () => {
    const column = columns(MAP);
    const at = (id: string) => column.get(`helsinki/${id}`) ?? -1;
    expect(at("MqttBroker/hsl")).toBeLessThan(at("DataSource/hfp"));
    expect(at("DataSource/hfp")).toBeLessThan(at("Pipeline/hfp-ingest"));
    expect(at("Pipeline/hfp-ingest")).toBeLessThan(at("Endpoint/ep-vehicles"));
    // The space and its endpoint point at each other: the loop does not push them apart forever.
    expect(at("ContextSpace/vehicles")).toBeLessThan(at("Pipeline/stop-delays"));
    expect(at("Pipeline/stop-delays")).toBeLessThan(at("ContextSpace/stops"));
    expect(Math.max(...column.values())).toBeLessThan(MAP.nodes.length);
  });

  it("places_no_two_cards_on_each_other", () => {
    const { placed, width, height } = layout(MAP);
    const spots = new Set(placed.map((entry) => `${entry.x},${entry.y}`));
    expect(spots.size).toBe(MAP.nodes.length);
    expect(width).toBeGreaterThan(0);
    expect(height).toBeGreaterThan(0);
  });

  it("groups_the_phone_list_and_finds_the_missing_grant_and_the_targets", () => {
    expect(groups(MAP).map((entry) => entry.group)).toEqual(["sources", "pipelines", "endpoints", "spaces"]);
    expect(missingGrants(MAP).map((edge) => edge.to)).toEqual(["helsinki/Endpoint/ep-stops"]);
    expect(missingGrants(MAP, "helsinki/Pipeline/hfp-ingest")).toEqual([]);
    expect(targets(MAP).map((endpoint) => endpoint.name)).toEqual(["ep-vehicles", "ep-stops"]);
    expect(targetUrn("hel.fi", targets(MAP)[0])).toBe("urn:ngsi-ld:Endpoint:hel.fi:vehicles:ep-vehicles");
  });

  it("names_another_projects_endpoint_but_never_offers_it_as_a_target", () => {
    const foreign = node("Endpoint", "ep-roads", { id: "espoo/Endpoint/ep-roads", project: "espoo", opaque: true });
    expect(targets({ ...MAP, nodes: [...MAP.nodes, foreign] }).map((endpoint) => endpoint.name)).not.toContain(
      "ep-roads",
    );
  });
});

describe("send to a space (F-R2UI3-004)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("names the target under the project's Organization domain, not the installation's", async () => {
    const chosen: Record<string, unknown>[] = [];
    renderPage(<SendToSpace project="helsinki" map={MAP} node={MAP.nodes[1]} onNewPipeline={(form) => chosen.push(form)} />, {
      path: "/projects/helsinki/pipelines",
      branding: { orgDomain: "installation.example" },
      answer: (url) => {
        if (url.pathname === "/api/v1/projects") {
          return json({ items: [{ name: "helsinki", organization: "espoo" }], organizations: [{ name: "espoo" }] });
        }
        if (url.pathname === "/api/v1/projects/org/organizations") {
          return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Organization", metadata: { name: "espoo" }, spec: { domain: "espoo.fi" } }]));
        }
        return undefined;
      },
    });
    const target = await screen.findByLabelText(en.pipelines.map.sendTarget);
    await userEvent.selectOptions(target, within(target).getAllByRole("option")[1]);
    await waitFor(async () => {
      chosen.length = 0;
      await userEvent.click(screen.getByRole("button", { name: en.pipelines.map.send }));
      expect(String(chosen[0]?.targetEndpoint)).toMatch(/^urn:ngsi-ld:Endpoint:espoo\.fi:/);
    });
  });
});

describe("the flow map page", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    window.history.pushState({}, "", "/");
  });

  it("shows_the_map_tab_with_the_missing_grant_and_its_fix", async () => {
    const { container } = await renderRoute({ path: PATH, answer: answering(MAP) });
    expect(await screen.findByRole("tab", { name: en.pipelines.map.view.map })).toHaveAttribute("aria-selected", "true");
    // The tab's panel is the map view itself.
    expect(FlowMapView).toBeTypeOf("function");
    const alert = await screen.findByText(/1 path lacks the grant it needs/);
    expect(alert.closest("[role]")).toHaveTextContent(
      "add a Policy on space stops that grants the ServiceAccount pipelines upsertBatch",
    );
    await expectAxeClean(container);
  });

  it("opens_a_pipelines_detail_with_its_grants_and_actions", async () => {
    await renderRoute({ path: PATH, answer: answering(MAP) });
    const cards = await screen.findAllByRole("button", { name: /^Pipeline: stop-delays\./ });
    await userEvent.click(cards[0]);
    const detail = await screen.findByRole("heading", { level: 2, name: "stop-delays" });
    const card = detail.closest("div.flex.flex-col.gap-4") as HTMLElement;
    expect(within(card).getByText(en.pipelines.map.grant.missing)).toBeInTheDocument();
    expect(within(card).getByText(/Policy vehicles-pipelines/)).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: en.pipelines.map.openRuns })).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: en.pipelines.resume })).toBeInTheDocument();
    await waitFor(() => expect(detail).toHaveFocus());
  });

  it("sends_a_data_source_to_a_space_through_the_prefilled_editor", async () => {
    await renderRoute({ path: PATH, answer: answering(MAP) });
    const cards = await screen.findAllByRole("button", { name: /^Data source: hfp\. mqtt/ });
    await userEvent.click(cards[0]);
    const target = await screen.findByLabelText(en.pipelines.map.sendTarget);
    expect(within(target).getAllByRole("option").map((option) => option.textContent)).toContain(
      "ep-stops (space stops)",
    );
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.map.send }));
    const page = await findFormPage();
    // The source the map was on is the editor's source already.
    expect(await within(page).findByDisplayValue("hfp")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/helsinki/pipelines/new");
  });

  it("pauses_a_running_pipeline_as_a_change_checked_first", async () => {
    const sent: { dryRun: boolean; enabled: unknown }[] = [];
    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "hfp-ingest", namespace: "helsinki" },
      spec: { enabled: true, source: { dataSourceRef: "hfp" } },
    };
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (path.endsWith("/flow-map")) return jsonResponse(MAP);
        if (path.endsWith("/pipelines/hfp-ingest") && request.method === "GET") return jsonResponse(manifest);
        if (path.endsWith("/pipelines/hfp-ingest") && request.method === "PUT") {
          const dryRun = new URL(request.url).searchParams.get("dryRun") === "All";
          return (request.json() as Promise<{ spec: { enabled?: unknown } }>).then((body) => {
            sent.push({ dryRun, enabled: body.spec.enabled });
            return dryRun
              ? jsonResponse({ valid: true, verdict: { ok: true, findings: [] } })
              : jsonResponse({
                  apiVersion: "joinedcontext.com/v1alpha1",
                  kind: "Change",
                  metadata: { name: "chg-00000042", namespace: "helsinki" },
                  status: { lane: "green", phase: "Applied" },
                });
          });
        }
        return undefined;
      },
    });
    // The pipeline's buttons are this component's.
    expect(PipelineActions).toBeTypeOf("function");
    const cards = await screen.findAllByRole("button", { name: /^Pipeline: hfp-ingest\./ });
    await userEvent.click(cards[0]);
    await userEvent.click(await screen.findByRole("button", { name: en.pipelines.pause }));
    expect(await screen.findByText(/chg-00000042/)).toBeInTheDocument();
    expect(sent).toEqual([
      { dryRun: true, enabled: false },
      { dryRun: false, enabled: false },
    ]);
  });

  it("asks_for_a_data_source_first_on_a_broker_that_has_none", async () => {
    const lone: FlowMap = { project: "helsinki", nodes: [node("MqttBroker", "hsl")], edges: [] };
    await renderRoute({ path: PATH, answer: answering(lone) });
    // A broker's "send to a space" is this component's.
    expect(SendToSpace).toBeTypeOf("function");
    const cards = await screen.findAllByRole("button", { name: /^MQTT broker: hsl/ });
    await userEvent.click(cards[0]);
    expect(await screen.findByText(en.pipelines.map.brokerFirst)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.map.newDataSource }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/datasources"));
  });

  it("says_what_to_do_when_nothing_flows_yet", async () => {
    await renderRoute({ path: PATH, answer: answering({ project: "helsinki", nodes: [], edges: [] }) });
    expect(await screen.findByText(en.pipelines.map.empty)).toBeInTheDocument();
    expect(screen.getByText(en.pipelines.map.emptyHint)).toBeInTheDocument();
  });

  it("says_why_the_map_could_not_be_read", async () => {
    await renderRoute({
      path: PATH,
      answer: (path) => (path.endsWith("/flow-map") ? problem(503, "The mirror is not loaded yet.") : undefined),
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("The mirror is not loaded yet.");
  });
});
