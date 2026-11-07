/**
 * T-3088: a pipeline writing through several Endpoints (PL-55) had its second output in YAML
 * only. The canvas now stands every output in the right column, fed by the tail of the lane, and
 * a second output is added, edited and removed in place; the manifest carries what was edited.
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
import { addOutput, paintOf, removeOutput, toFlow } from "../src/pages/pipelines/PipelineFlow";
import { PipelineStudio } from "../src/pages/pipelines/PipelineStudio";
import type { Trace } from "../src/pages/pipelines/PipelineTest";

vi.mock("../src/branding", () => ({
  useBranding: () => ({ orgDomain: "hel.fi" }),
}));

const FIRST = "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes";
const SECOND = "urn:ngsi-ld:Endpoint:hel.fi:archive:ep-archive";

const form: PipelineForm = {
  name: "bikes",
  class: "auto",
  source: { dataSourceRef: "feed-bikes" },
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: FIRST,
  output: { type: "BikeHireDockingStation", mode: "upsert" },
  moreOutputs: [{ targetEndpoint: SECOND, type: "BikeHireDockingStation", mode: "update-attrs" }],
};

describe("several outputs on the canvas", () => {
  it("stands every output after the lane, each fed by its tail", () => {
    const { nodes, edges } = toFlow(form);
    expect(nodes.map((node) => node.id)).toEqual(["source", "compute", "output", "output-0"]);
    expect(edges).toEqual([
      { from: "source", to: "compute" },
      { from: "compute", to: "output" },
      { from: "compute", to: "output-0" },
    ]);
    expect(nodes[3].summary).toBe("BikeHireDockingStation · update-attrs · ep-archive");
  });

  it("feeds every output from every source when the lane is empty", () => {
    const { edges } = toFlow({ ...form, compute: undefined, moreSources: [{ dataSourceRef: "feed-docks" }] });
    expect(edges).toEqual([
      { from: "source", to: "output" },
      { from: "source", to: "output-0" },
      { from: "source-0", to: "output" },
      { from: "source-0", to: "output-0" },
    ]);
  });

  it("paints a second output with the first one's figures, which are the same messages", () => {
    const trace = { input: { events: 2, bytes: 9 }, mapping: [{}, {}], validation: [{ index: 0, ok: true, problems: [] }, { index: 1, ok: false, problems: ["bad"] }], errors: [] } as unknown as Trace;
    const paint = paintOf(trace, toFlow(form).nodes);
    expect(paint["output-0"]).toEqual(paint.output);
    expect(paint.output.state).toBe("error");
  });

  it("adds an output that writes upsert until told otherwise, and removes only the one asked", () => {
    const added = addOutput(form);
    expect(added.id).toBe("output-1");
    expect(added.form.moreOutputs?.[1]).toEqual({ mode: "upsert" });
    expect(removeOutput(added.form, 0).moreOutputs).toEqual([{ mode: "upsert" }]);
  });

  it("writes the outputs into the manifest in order and reads them back", () => {
    const manifest = toEnvelope("helsinki", form);
    expect((manifest.spec as { outputs: unknown[] }).outputs).toEqual([
      { targetEndpoint: FIRST, type: "BikeHireDockingStation", mode: "upsert" },
      { targetEndpoint: SECOND, type: "BikeHireDockingStation", mode: "update-attrs" },
    ]);
    expect(toForm(manifest).moreOutputs).toEqual(form.moreOutputs);
    // An emptied list is kept empty, so a removal reaches the manifest.
    const removed = toEnvelope("helsinki", { ...form, moreOutputs: [] }, manifest);
    expect((removed.spec as { outputs: unknown[] }).outputs).toHaveLength(1);
  });
});

describe("a second output edited in place", () => {
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
    { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "ep-bikes", namespace: "helsinki" }, spec: { contextSpaceRef: "mobility" } },
    { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "ep-archive", namespace: "helsinki" }, spec: { contextSpaceRef: "archive" } },
  ];

  function Host({ seen }: { seen: (form: PipelineForm) => void }) {
    const [draft, setDraft] = useState<PipelineForm>({ ...form, moreOutputs: undefined });
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

  it("adds an output from the palette, picks its Endpoint and mode, and takes it away again", async () => {
    const seen = vi.fn();
    render(<Host seen={seen} />);
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.flow.addOutput }));
    expect(screen.getByTestId("flow-node-output-0")).toBeInTheDocument();
    const editor = screen.getByTestId("flow-node-editor-output");
    expect(editor).toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText(new RegExp(`^${en.pipelines.flow.outputWrites}`)), SECOND);
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.field.outputMode), "update-attrs");
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({ moreOutputs: [{ targetEndpoint: SECOND, mode: "update-attrs" }] }),
    );

    await userEvent.click(screen.getByTestId("flow-output-remove"));
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ moreOutputs: [] }));
    expect(screen.queryByTestId("flow-node-output-0")).not.toBeInTheDocument();
  });

  it("removes a second output with Delete and never the first one", async () => {
    const seen = vi.fn();
    render(<Host seen={seen} />);
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.flow.addOutput }));
    screen.getByTestId("flow-node-output").focus();
    await userEvent.keyboard("{Delete}");
    expect(screen.getByTestId("flow-node-output")).toBeInTheDocument();
    screen.getByTestId("flow-node-output-0").focus();
    await userEvent.keyboard("{Delete}");
    expect(screen.queryByTestId("flow-node-output-0")).not.toBeInTheDocument();
  });
});
