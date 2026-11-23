/**
 * T-7680: the flow map's choices and actions. Narrowing to one space asks the server for that
 * space and writes it into the address; a map that failed offers a retry; a missing grant leads
 * to its pipeline; another project's node shows its owner and offers nothing; an Endpoint, a
 * space and a pipeline each open their page; a refused pause says why; a broker with a data
 * source sends that source to a space.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import type { FlowMap, FlowNode } from "../src/pages/pipelines/flowMap";
import { findFormPage } from "./formPage";
import { jsonResponse, problem, renderRoute } from "./pageHarness";

const PATH = "/projects/helsinki/pipelines?view=map";
const m = en.pipelines.map;

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
    node("DataSource", "hfp", { sourceType: "mqtt" }),
    node("Pipeline", "hfp-ingest", { role: "ingest", phase: "Live", enabled: true }),
    node("Endpoint", "ep-vehicles", { space: "vehicles", slug: "vehicles" }),
    node("ContextSpace", "vehicles"),
    node("Pipeline", "stop-delays", { role: "derived", phase: "Live", enabled: true }),
    node("Endpoint", "ep-stops", { space: "stops" }),
    node("ContextSpace", "stops"),
    { id: "espoo/Endpoint/ep-buses", kind: "Endpoint", name: "ep-buses", project: "espoo", opaque: true },
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
      grant: { state: "missing", endpoint: "ep-stops", operation: "upsertBatch", fix: "add a Policy on space stops for the pipelines" },
    },
    { from: "helsinki/Endpoint/ep-stops", to: "helsinki/ContextSpace/stops", relation: "stores" },
    {
      from: "espoo/Endpoint/ep-buses",
      to: "helsinki/Pipeline/stop-delays",
      relation: "reads",
      grant: { state: "external", endpoint: "ep-buses", operation: "queryEntity" },
    },
  ],
};

const EDITOR = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "editor", binding: "editors", rule: { kinds: ["Pipeline", "DataSource"], verbs: ["read", "propose"] } }],
};

const open = async (name: RegExp) => {
  const cards = await screen.findAllByRole("button", { name });
  await userEvent.click(cards[0]);
};

describe("the flow map's choices and actions", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it("narrows to one space, asks for that space and keeps it in the address", async () => {
    const asked: string[] = [];
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (!path.endsWith("/flow-map")) return undefined;
        const space = new URL(request.url).searchParams.get("space") ?? "";
        asked.push(space);
        return jsonResponse(space === "stops" ? { project: "helsinki", nodes: [], edges: [] } : MAP);
      },
    });
    const select = await screen.findByLabelText(m.space);
    await waitFor(() => expect(within(select).getByRole("option", { name: "stops" })).toBeInTheDocument());
    await userEvent.selectOptions(select, "stops");
    expect(await screen.findByText(m.emptySpace)).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("space")).toBe("stops");
    expect(asked).toContain("stops");
    await userEvent.selectOptions(screen.getByLabelText(m.space), m.allSpaces);
    expect(await screen.findByText(/1 path lacks the grant it needs/)).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).has("space")).toBe(false);
  });

  it("offers a retry when the map could not be read, and draws it once it can", async () => {
    let fail = true;
    await renderRoute({
      path: PATH,
      answer: (path) => (path.endsWith("/flow-map") ? (fail ? problem(503, "The mirror is not loaded yet.") : jsonResponse(MAP)) : undefined),
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("The mirror is not loaded yet.");
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(/1 path lacks the grant it needs/)).toBeInTheDocument();
  });

  it("opens the pipeline a missing grant belongs to, and closes its detail", async () => {
    await renderRoute({ path: PATH, answer: (path) => (path.endsWith("/flow-map") ? jsonResponse(MAP) : undefined) });
    const alert = (await screen.findByText(/1 path lacks the grant it needs/)).closest("[role]") as HTMLElement;
    await userEvent.click(within(alert).getByRole("button", { name: "stop-delays" }));
    const heading = await screen.findByRole("heading", { level: 2, name: "stop-delays" });
    expect(screen.getByText(m.grant.external, { selector: "li *, span" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: m.close }));
    await waitFor(() => expect(heading).not.toBeInTheDocument());
  });

  it("names another project's node by its owner and offers nothing to do with it", async () => {
    await renderRoute({ path: PATH, answer: (path) => (path.endsWith("/flow-map") ? jsonResponse(MAP) : undefined) });
    await open(/^Endpoint: ep-buses/);
    const card = (await screen.findByRole("heading", { level: 2, name: "ep-buses" })).closest("div.flex.flex-col.gap-4") as HTMLElement;
    expect(within(card).getByText("Project espoo; you cannot read its details")).toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: m.openEndpoint })).toBeNull();
  });

  it("opens an Endpoint's page and a space's page", async () => {
    await renderRoute({ path: PATH, answer: (path) => (path.endsWith("/flow-map") ? jsonResponse(MAP) : undefined) });
    await open(/^Endpoint: ep-vehicles/);
    await userEvent.click(await screen.findByRole("button", { name: m.openEndpoint }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/endpoints/ep-vehicles"));
    window.history.pushState({}, "", PATH);
    await renderRoute({ path: PATH, answer: (path) => (path.endsWith("/flow-map") ? jsonResponse(MAP) : undefined) });
    await open(/^Space: vehicles/);
    await userEvent.click(await screen.findByRole("button", { name: m.openSpace }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces/vehicles"));
  });

  it.each([
    [m.openPipeline, "/projects/helsinki/pipelines/hfp-ingest", ""],
    [m.openRuns, "/projects/helsinki/pipelines/hfp-ingest", "#runs"],
    [m.openRejected, "/projects/helsinki/pipelines/hfp-ingest", "#rejected"],
    [m.edit, "/projects/helsinki/pipelines/hfp-ingest/edit", ""],
  ])("opens %s of a pipeline", async (label, pathname, hash) => {
    await renderRoute({ path: PATH, answer: (path) => (path.endsWith("/flow-map") ? jsonResponse(MAP) : undefined), permissions: EDITOR });
    await open(/^Pipeline: hfp-ingest\./);
    const button = await screen.findByRole("button", { name: label });
    await waitFor(() => expect(button).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(button);
    await waitFor(() => expect(window.location.pathname).toBe(pathname));
    expect(window.location.hash).toBe(hash);
  });

  it("says why a pause was refused", async () => {
    await renderRoute({
      path: PATH,
      permissions: EDITOR,
      answer: (path, request) => {
        if (path.endsWith("/flow-map")) return jsonResponse(MAP);
        if (path.endsWith("/pipelines/hfp-ingest") && request.method === "GET") {
          return jsonResponse({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Pipeline", metadata: { name: "hfp-ingest", namespace: "helsinki" }, spec: {} });
        }
        if (request.method === "PUT") return problem(403, "you may not propose Pipelines in helsinki");
        return undefined;
      },
    });
    await open(/^Pipeline: hfp-ingest\./);
    const pause = await screen.findByRole("button", { name: en.pipelines.pause });
    await waitFor(() => expect(pause).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(pause);
    expect(await screen.findByRole("alert")).toHaveTextContent("you may not propose Pipelines in helsinki");
  });

  it("sends a broker's data source to a space", async () => {
    await renderRoute({ path: PATH, answer: (path) => (path.endsWith("/flow-map") ? jsonResponse(MAP) : undefined), permissions: EDITOR });
    await open(/^MQTT broker: hsl/);
    await userEvent.click(await screen.findByRole("button", { name: m.send }));
    const page = await findFormPage();
    expect(await within(page).findByDisplayValue("hfp")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/projects/helsinki/pipelines/new");
  });
});
