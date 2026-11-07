/**
 * T-0643: Pipeline flow canvas (UI-49, PL-31, PL-32, PL-43).
 * Asserts projection from PipelineForm to flow nodes and edges, paint logic from trace,
 * stage mutators, and keyboard / palette interactions.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import i18n from "../src/i18n";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { toEnvelope, toForm } from "../src/pages/pipelines/PipelineEditor";
import {
  PipelineFlow,
  paintOf,
  setComputeKind,
  toFlow,
} from "../src/pages/pipelines/PipelineFlow";
import { PipelineStudio } from "../src/pages/pipelines/PipelineStudio";
import type { Trace } from "../src/pages/pipelines/PipelineTest";

vi.mock("../src/branding", () => ({
  useBranding: () => ({ orgDomain: "hel.fi" }),
}));

describe("PipelineFlow unit logic", () => {
  it("toFlow of undefined / empty form yields source and output with pass-through edge", () => {
    const { nodes, edges } = toFlow(undefined);
    expect(nodes).toHaveLength(2);
    const source = nodes.find((n) => n.id === "source");
    const compute = nodes.find((n) => n.id === "compute");
    const output = nodes.find((n) => n.id === "output");

    expect(source?.present).toBe(true);
    expect(source?.kind).toBe("none");
    expect(compute).toBeUndefined();
    expect(output?.present).toBe(true);
    expect(output?.kind).toBe("output");

    expect(edges).toEqual([{ from: "source", to: "output" }]);
  });

  it("toFlow of a form with compute.kind bloblang yields three present nodes and two edges", () => {
    const form: PipelineForm = {
      source: { dataSourceRef: "feed-bikes" },
      compute: { kind: "bloblang", bloblang: "root = this\nroot.active = true" },
      targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes",
      output: { type: "Vehicle", mode: "upsert" },
    };

    const { nodes, edges } = toFlow(form);
    const source = nodes.find((n) => n.id === "source");
    const compute = nodes.find((n) => n.id === "compute");
    const output = nodes.find((n) => n.id === "output");

    expect(source?.present).toBe(true);
    expect(source?.kind).toBe("datasource");
    expect(source?.summary).toContain("feed-bikes");

    expect(compute?.present).toBe(true);
    expect(compute?.kind).toBe("bloblang");
    expect(compute?.summary).toBe("root = this");

    expect(output?.present).toBe(true);
    expect(output?.summary).toContain("Vehicle");
    expect(output?.summary).toContain("upsert");

    expect(edges).toEqual([
      { from: "source", to: "compute" },
      { from: "compute", to: "output" },
    ]);
  });

  it("toFlow formats summaries for space source and other compute kinds", () => {
    const spaceForm: PipelineForm = {
      source: { endpointRef: "public-air", query: { type: "AirQualityObserved" } },
      compute: { kind: "wasm", module: "filter_aq", function: "apply" },
      targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:public-air",
    };
    const { nodes } = toFlow(spaceForm);
    const source = nodes.find((n) => n.id === "source");
    const compute = nodes.find((n) => n.id === "compute");

    expect(source?.kind).toBe("space");
    expect(source?.summary).toContain("public-air");
    expect(source?.summary).toContain("AirQualityObserved");
    expect(compute?.summary).toBe("filter_aq.apply");
  });

  it("setComputeKind updates compute and prunes unused stage fields", () => {
    const initial: PipelineForm = {
      compute: {
        kind: "bloblang",
        bloblang: "root = this",
        mappingRef: "map-stewards",
        module: "mod_calc",
        function: "exec",
      },
    };

    const asMapping = setComputeKind(initial, "mapping");
    expect(asMapping.compute?.kind).toBe("mapping");
    expect(asMapping.compute?.mappingRef).toBe("map-stewards");
    expect(asMapping.compute?.bloblang).toBeUndefined();
    expect(asMapping.compute?.module).toBeUndefined();

    const asWasm = setComputeKind(initial, "wasm");
    expect(asWasm.compute?.kind).toBe("wasm");
    expect(asWasm.compute?.module).toBe("mod_calc");
    expect(asWasm.compute?.function).toBe("exec");
    expect(asWasm.compute?.bloblang).toBeUndefined();

    const removed = setComputeKind(initial, null);
    expect(removed.compute).toBeUndefined();

    const restored = setComputeKind(removed, "bloblang");
    expect(restored.compute?.kind).toBe("bloblang");
  });

  it("manifest round-trip through toEnvelope and toForm preserves stage count after setComputeKind", () => {
    const form: PipelineForm = {
      name: "weather-ingest",
      class: "resident",
      source: { dataSourceRef: "mqtt-weather" },
      compute: { kind: "bloblang", bloblang: "root = this" },
      targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:weather:ep-weather",
    };

    const envelope = toEnvelope("helsinki", form);
    const back = toForm(envelope as Manifest);
    expect(toFlow(back).nodes.filter((n) => n.present)).toHaveLength(3);

    const passThrough = setComputeKind(back, null);
    const passEnvelope = toEnvelope("helsinki", passThrough);
    const passBack = toForm(passEnvelope as Manifest);
    expect(toFlow(passBack).nodes.filter((n) => n.present)).toHaveLength(2);
  });

  it("paintOf paints compute error, output skipped and source ok on mapping error", () => {
    const nodes = toFlow({ compute: { kind: "bloblang" } }).nodes;
    const trace: Trace = {
      input: { events: 12, bytes: 1024 },
      mapping: [],
      validation: [],
      errors: [{ stage: "mapping", line: 3, message: "failed to evaluate root" }],
    };
    const paint = paintOf(trace, nodes);
    expect(paint.source.state).toBe("ok");
    expect(paint.source.eventsIn).toBe(12);
    expect(paint.source.eventsOut).toBe(12);

    expect(paint.compute.state).toBe("error");
    expect(paint.compute.eventsIn).toBe(12);
    expect(paint.compute.eventsOut).toBe(0);
    expect(paint.compute.error).toBe("failed to evaluate root");

    expect(paint.output.state).toBe("skipped");
  });

  it("paintOf paints output error on validation failure when mapping succeeded", () => {
    const nodes = toFlow({ compute: { kind: "bloblang" } }).nodes;
    const trace: Trace = {
      input: { events: 4, bytes: 400 },
      mapping: [{ id: "urn:ngsi-ld:Invalid:1" }],
      validation: [{ index: 0, ok: false, problems: ["missing @context attribute"] }],
      errors: [],
    };
    const paint = paintOf(trace, nodes);
    expect(paint.source.state).toBe("ok");
    expect(paint.compute.state).toBe("ok");
    expect(paint.compute.eventsIn).toBe(4);
    expect(paint.compute.eventsOut).toBe(1);

    expect(paint.output.state).toBe("error");
    expect(paint.output.eventsIn).toBe(1);
    expect(paint.output.eventsOut).toBe(0);
    expect(paint.output.error).toContain("missing @context attribute");
  });

  it("paintOf returns all idle when trace is null", () => {
    const nodes = toFlow(undefined).nodes;
    const paint = paintOf(null, nodes);
    expect(paint.source.state).toBe("idle");
    expect(paint.compute.state).toBe("idle");
    expect(paint.output.state).toBe("idle");
  });
});

describe("PipelineFlow component rendering & interactions", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ items: [] }), {
            headers: { "Content-Type": "application/json" },
          }),
        ),
      ),
    );
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("clicking palette stage updates form compute kind", async () => {
    const onChange = vi.fn();
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={{ name: "flow-test" }}
          onChange={onChange}
          trace={null}
          selected={null}
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    const mappingBtn = screen.getByTestId("palette-mapping");
    await userEvent.click(mappingBtn);

    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        compute: expect.objectContaining({ kind: "mapping" }),
      }),
    );
    expect(onSelect).toHaveBeenCalledWith("compute");
  });

  /// T-1135: the canvas takes a stage dropped on it, which is the other half of the palette.
  it("dropping a palette stage on the canvas adds it and selects it", () => {
    const onChange = vi.fn();
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={{ name: "flow-test" }}
          onChange={onChange}
          trace={null}
          selected={null}
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    const canvas = screen.getByTestId("flow-canvas");
    const transfer = { getData: () => "bloblang", dropEffect: "" };
    fireEvent.dragOver(canvas, { dataTransfer: transfer });
    fireEvent.drop(canvas, { dataTransfer: transfer });

    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ compute: expect.objectContaining({ kind: "bloblang" }) }),
    );
    expect(onSelect).toHaveBeenCalledWith("compute");
  });

  it("ignores anything dropped on the canvas that is not a stage", () => {
    const onChange = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={{ name: "flow-test" }}
          onChange={onChange}
          trace={null}
          selected={null}
          onSelect={vi.fn()}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    fireEvent.drop(screen.getByTestId("flow-canvas"), {
      dataTransfer: { getData: () => "https://example.test/a-link", dropEffect: "" },
    });
    expect(onChange).not.toHaveBeenCalled();
  });

  /// The canvas is reachable without a mouse: every node is a button, and the arrows walk them.
  it("walks the nodes with the arrow keys, and stops at both ends", () => {
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={{ name: "flow-test", compute: { kind: "bloblang", bloblang: "root = this" } }}
          onChange={vi.fn()}
          trace={null}
          selected="compute"
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    fireEvent.keyDown(screen.getByTestId("flow-node-compute"), { key: "ArrowRight" });
    expect(onSelect).toHaveBeenLastCalledWith("output");
    fireEvent.keyDown(screen.getByTestId("flow-node-compute"), { key: "ArrowLeft" });
    expect(onSelect).toHaveBeenLastCalledWith("source");

    // The ends hold: there is nothing left of the source or right of the output.
    fireEvent.keyDown(screen.getByTestId("flow-node-source"), { key: "ArrowLeft" });
    expect(onSelect).toHaveBeenLastCalledWith("source");
    fireEvent.keyDown(screen.getByTestId("flow-node-output"), { key: "ArrowRight" });
    expect(onSelect).toHaveBeenLastCalledWith("output");
  });

  it("selects a node with Enter and with the space bar, as a button does", () => {
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={{ name: "flow-test" }}
          onChange={vi.fn()}
          trace={null}
          selected={null}
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    fireEvent.keyDown(screen.getByTestId("flow-node-source"), { key: "Enter" });
    fireEvent.keyDown(screen.getByTestId("flow-node-output"), { key: " " });
    expect(onSelect).toHaveBeenNthCalledWith(1, "source");
    expect(onSelect).toHaveBeenNthCalledWith(2, "output");
  });

  it("pressing Delete on selected compute node removes stage", () => {
    const onChange = vi.fn();
    const onSelect = vi.fn();
    const form: PipelineForm = {
      compute: { kind: "bloblang", bloblang: "root = this" },
    };

    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={form}
          onChange={onChange}
          trace={null}
          selected="compute"
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    const computeNode = screen.getByTestId("flow-node-compute");
    fireEvent.keyDown(computeNode, { key: "Delete" });

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][0]).not.toHaveProperty("compute");
  });

  it("navigates selection with arrow keys along stage order", () => {
    const onSelect = vi.fn();
    const form: PipelineForm = {
      compute: { kind: "bloblang" },
    };

    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={form}
          onChange={vi.fn()}
          trace={null}
          selected="source"
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    const sourceNode = screen.getByTestId("flow-node-source");
    fireEvent.keyDown(sourceNode, { key: "ArrowRight" });
    expect(onSelect).toHaveBeenCalledWith("compute");
  });

  it("renders node data-state attributes according to trace paint", () => {
    const form: PipelineForm = { compute: { kind: "bloblang" } };
    const trace: Trace = {
      input: { events: 2, bytes: 200 },
      mapping: [],
      validation: [],
      errors: [{ stage: "mapping", message: "syntax error" }],
    };

    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={form}
          onChange={vi.fn()}
          trace={trace}
          selected={null}
          onSelect={vi.fn()}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );

    expect(screen.getByTestId("flow-node-source")).toHaveAttribute("data-state", "ok");
    expect(screen.getByTestId("flow-node-compute")).toHaveAttribute("data-state", "error");
    expect(screen.getByTestId("flow-node-output")).toHaveAttribute("data-state", "skipped");
  });

  it("switches between Flow and Form view in PipelineStudio", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const draft: PipelineForm = {
      class: "resident",
      source: { dataSourceRef: "mqtt-feed" },
      compute: { kind: "bloblang", bloblang: "root = this" },
      targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:ep",
    };

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <PipelineStudio
            project="helsinki"
            draft={draft}
            onChange={vi.fn()}
            dataSources={[]}
            endpoints={[]}
            toManifest={vi.fn()}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    expect(screen.getByTestId("flow-canvas")).toBeInTheDocument();

    const formTab = screen.getByTestId("studio-view-form");
    await userEvent.click(formTab);

    expect(screen.queryByTestId("flow-canvas")).not.toBeInTheDocument();

    const flowTab = screen.getByTestId("studio-view-flow");
    await userEvent.click(flowTab);

    expect(screen.getByTestId("flow-canvas")).toBeInTheDocument();
  });
});
