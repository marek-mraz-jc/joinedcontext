/**
 * The first source and the first output are edited on their own canvas nodes, with the blocks
 * the extra ones open, and neither can be removed there (T-3978, PL-52, PL-68).
 */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { PipelineStudio } from "../src/pages/pipelines/PipelineStudio";

vi.mock("../src/branding", () => ({
  useBranding: () => ({ orgDomain: "hel.fi" }),
}));

const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes";
const dataSources: Manifest[] = [
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "DataSource", metadata: { name: "feed-bikes", namespace: "helsinki" }, spec: {} },
];
const endpoints: Manifest[] = [
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "ep-bikes", namespace: "helsinki" }, spec: { contextSpaceRef: "mobility" } },
];

function Host({ start, seen }: { start: PipelineForm; seen: (form: PipelineForm) => void }) {
  const [draft, setDraft] = useState<PipelineForm>(start);
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
          dataSources={dataSources}
          endpoints={endpoints}
          toManifest={vi.fn()}
        />
      </I18nextProvider>
    </QueryClientProvider>
  );
}

describe("the first source and output on the canvas", () => {
  it("edits the first source on its node and sets `source`, with nothing to remove", async () => {
    const seen = vi.fn();
    render(<Host start={{ name: "bikes", compute: { kind: "bloblang", bloblang: "root = this" } }} seen={seen} />);
    await userEvent.click(screen.getByTestId("flow-node-source"));
    const editor = screen.getByTestId("flow-node-editor-source");
    expect(screen.queryByTestId("flow-source-remove")).not.toBeInTheDocument();

    await userEvent.selectOptions(screen.getByTestId("flow-source-ref"), "DataSource:feed-bikes");
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({ source: expect.objectContaining({ dataSourceRef: "feed-bikes", endpointRef: undefined }) }),
    );
    expect(seen.mock.lastCall?.[0].moreSources).toBeUndefined();
    expect(editor).toBeInTheDocument();
  });

  it("edits the first output on its node: the target and the mode land where the manifest keeps them", async () => {
    const seen = vi.fn();
    render(<Host start={{ name: "bikes", source: { dataSourceRef: "feed-bikes" }, output: { type: "BikeHireDockingStation" } }} seen={seen} />);
    await userEvent.click(screen.getByTestId("flow-node-output"));
    expect(screen.getByTestId("flow-node-editor-output")).toBeInTheDocument();
    expect(screen.queryByTestId("flow-output-remove")).not.toBeInTheDocument();

    await userEvent.selectOptions(screen.getByTestId("flow-output-target"), TARGET);
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.field.outputMode), "update-attrs");
    const last = seen.mock.lastCall?.[0] as PipelineForm;
    expect(last.targetEndpoint).toBe(TARGET);
    expect(last.output).toEqual({ type: "BikeHireDockingStation", mode: "update-attrs" });
    expect(last.moreOutputs).toBeUndefined();
  });
});
