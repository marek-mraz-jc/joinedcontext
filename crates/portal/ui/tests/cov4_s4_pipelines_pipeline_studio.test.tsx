// T-7706: the pipeline studio's remaining paths: a route's own step edited and removed, a tapped wire named in the debug sidebar, the pipeline's secret variables on a step, a sample without a model, the aggregate, and a KPI test whose request fails.
import { useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { PipelineStudio } from "../src/pages/pipelines/PipelineStudio";

vi.mock("../src/branding", () => ({
  useBranding: () => ({ orgDomain: "hel.fi" }),
}));

const LANE = "urn:ngsi-ld:Endpoint:hel.fi:air:ep-air";
const ALERTS = "urn:ngsi-ld:Endpoint:hel.fi:alerts:ep-alerts";
const API = "joinedcontext.com/v1alpha1";

const withRoute: PipelineForm = {
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
      steps: [{ processor: { jq: { query: ".id" } } }],
      outputs: [{ targetEndpoint: ALERTS, mode: "upsert" }],
    },
  ],
};

const endpoint = (name: string, space: string, slug?: string): Manifest => ({
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space, ...(slug ? { slug, audience: "public" } : {}) },
});

function stubFetch(respond: (url: URL, method: string) => Response | undefined) {
  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
  const asked: { url: URL; method: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      asked.push({ url, method });
      const own = respond(url, method);
      return own ? Promise.resolve(own) : Promise.resolve(json({ apiVersion: API, kind: "List", items: [] }));
    }),
  );
  return asked;
}

function Studio({
  initial,
  seen = () => undefined,
  endpoints = [],
  toManifest,
}: {
  initial: PipelineForm;
  seen?: (form: PipelineForm) => void;
  endpoints?: Manifest[];
  toManifest?: (form: PipelineForm) => unknown;
}) {
  const [draft, setDraft] = useState(initial);
  const [client] = useState(() => new QueryClient({ defaultOptions: { queries: { retry: false } } }));
  return (
    <QueryClientProvider client={client}>
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
          toManifest={toManifest as never}
        />
      </I18nextProvider>
    </QueryClientProvider>
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  stubFetch(() => undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a route's own step in the studio", () => {
  it("edits the step's block in place and removes it from its own button", async () => {
    const seen = vi.fn();
    render(<Studio initial={withRoute} seen={seen} />);
    await userEvent.click(screen.getByTestId("flow-node-route-0-step-0"));
    await userEvent.click(await screen.findByRole("tab", { name: "YAML" }));
    const block = screen.getByTestId("flow-step-yaml") as HTMLTextAreaElement;
    expect(block.value).toBe("jq:\n  query: .id\n");
    fireEvent.change(block, { target: { value: "jq:\n  query: .name\n" } });
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({ routes: [expect.objectContaining({ steps: [{ processor: { jq: { query: ".name" } } }] })] }),
    );
    await userEvent.click(screen.getByTestId("flow-step-remove"));
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ routes: [expect.objectContaining({ steps: [] })] }));
    expect(screen.queryByTestId("flow-node-route-0-step-0")).not.toBeInTheDocument();
  });
});

describe("the pipeline's variables on a step", () => {
  it("opens a step of a pipeline that names a secret variable", async () => {
    render(
      <Studio
        initial={{
          ...withRoute,
          routes: undefined,
          secretRefs: [{ name: "feed", key: "token", envVar: "FEED_TOKEN" }, { name: "plain", key: "x" }],
          processors: [{ step: { processor: { http: { url: "https://feed.example", headers: { Authorization: "${FEED_TOKEN}" } } } } }],
        }}
      />,
    );
    await userEvent.click(screen.getByTestId("flow-node-step-0"));
    await userEvent.click(await screen.findByRole("tab", { name: "YAML" }));
    expect((screen.getByTestId("flow-step-yaml") as HTMLTextAreaElement).value).toContain("Authorization: ${FEED_TOKEN}");
  });
});

describe("debugging on the studio's canvas", () => {
  it("names the nodes of a tapped wire in the sidebar's filter", async () => {
    render(
      <Studio
        initial={{ ...withRoute, routes: undefined, processors: [{ step: { processor: { log: {} } } }] }}
        toManifest={(form) => form}
      />,
    );
    await userEvent.click(screen.getByTestId("flow-node-compute"));
    await userEvent.click(screen.getByTestId("palette-debug"));
    const sidebar = screen.getByTestId("flow-debug");
    const filter = within(sidebar).getByLabelText(en.pipelines.debug.filter);
    const options = [...filter.querySelectorAll("option")].map((option) => option.textContent ?? "");
    expect(options.some((text) => text.startsWith(en.pipelines.flow.node.step))).toBe(true);
    expect(options.some((text) => text.startsWith(en.pipelines.flow.node.source))).toBe(true);
    expect(options.some((text) => text.startsWith(en.pipelines.flow.node.output))).toBe(true);
    expect(options.some((text) => text.startsWith(en.pipelines.flow.node.compute))).toBe(true);
  });
});

describe("a debug run in the studio", () => {
  it("lists what crossed the tapped wire and clears the list on request", async () => {
    const trace = {
      input: { events: 1, bytes: 20, sample: { id: "s1" } },
      mapping: [{ id: "s1", type: "Obs" }],
      validation: [{ index: 0, ok: true, problems: [] }],
      errors: [],
      stages: [{ step: 0, reached: 1, sample: { id: "s1" } }],
    };
    stubFetch((url, method) =>
      url.pathname.endsWith("/pipelines/test") && method === "POST"
        ? new Response(JSON.stringify(trace), { headers: { "Content-Type": "application/json" } })
        : undefined,
    );
    render(
      <Studio
        initial={{ ...withRoute, routes: undefined, processors: [{ step: { processor: { log: {} } } }] }}
        toManifest={(form) => ({ kind: "Pipeline", metadata: { name: form.name }, spec: form })}
      />,
    );
    await userEvent.click(screen.getByTestId("flow-node-compute"));
    await userEvent.click(screen.getByTestId("palette-debug"));
    const sample = new File([JSON.stringify([{ id: "s1" }])], "sample.json", { type: "application/json" });
    await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), sample);
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.test.run }));
    const sidebar = screen.getByTestId("flow-debug");
    expect(await within(sidebar).findByRole("list", { name: en.pipelines.debug.messages })).toBeInTheDocument();
    await userEvent.click(within(sidebar).getByRole("button", { name: en.pipelines.debug.clear }));
    expect(await within(sidebar).findByText(en.pipelines.debug.nothingYet)).toBeInTheDocument();
  });
});

describe("a space read without a model", () => {
  const SAMPLE_ROWS = [{ id: "urn:ngsi-ld:Obs:1", type: "Obs", pm10: 4, pm25: 2, "@context": "x" }];
  const space = { apiVersion: API, kind: "ContextSpace", metadata: { name: "air", namespace: "helsinki" }, spec: {} };
  const source = (type: string | undefined, endpointRef = "public-air"): PipelineForm => ({
    name: "agg",
    class: "auto",
    source: { endpointRef, ...(type ? { query: { type } } : {}) },
  });

  it("offers the sample's own keys as the attributes and aggregates the one picked", async () => {
    stubFetch((url) => {
      if (url.pathname.endsWith("/spaces")) return new Response(JSON.stringify({ apiVersion: API, kind: "List", items: [space] }), { headers: { "Content-Type": "application/json" } });
      if (url.pathname.includes("/ngsi-ld/v1/entities")) {
        return new Response(JSON.stringify(SAMPLE_ROWS), { headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" } });
      }
      return undefined;
    });
    const seen = vi.fn();
    render(<Studio initial={source("Obs")} seen={seen} endpoints={[endpoint("public-air", "air", "k7m2qz4tv6xh3n5jb2ryd3wcfa")]} />);
    const entities = await screen.findByRole("region", { name: en.pipelines.studio.entities });
    await userEvent.click(within(entities).getByRole("button", { name: en.pipelines.studio.loadSample }));
    await within(entities).findByRole("table", { name: en.pipelines.studio.sample });
    const process = screen.getByRole("region", { name: en.pipelines.studio.process });
    const attribute = within(process).getByRole("combobox", { name: en.pipelines.studio.attribute });
    await userEvent.click(attribute);
    await userEvent.click(await screen.findByRole("option", { name: "pm25" }));
    await userEvent.click(within(process).getByRole("button", { name: en.pipelines.studio.aggregate.sum }));
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ compute: expect.objectContaining({ kind: "bloblang", bloblang: expect.stringContaining("pm25") }) }));
  });

  it("writes no aggregate while the endpoint's space is unknown", async () => {
    const seen = vi.fn();
    render(<Studio initial={source("Obs", "unlisted")} seen={seen} />);
    const process = await screen.findByRole("region", { name: en.pipelines.studio.process });
    await userEvent.click(within(process).getByRole("button", { name: en.pipelines.studio.aggregate.sum }));
    expect(seen).not.toHaveBeenCalled();
  });
});

describe("the KPI test when it cannot be made at all", () => {
  it("says the test failed instead of showing a value", async () => {
    let explode = false;
    const endpoints = [endpoint("helsinki-all", "helsinki", "abc123"), endpoint("kpi-writer", "helsinki-kpi", "kpi456")];
    render(
      <Studio
        initial={{ name: "kpi-1", class: "auto" }}
        endpoints={endpoints}
        toManifest={(form) => {
          if (explode) throw new Error("the form cannot be written");
          return { kind: "Pipeline", metadata: { name: form.name }, spec: form };
        }}
      />,
    );
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(await screen.findByLabelText(en.pipelines.studio.kpi.endpoint), "helsinki-all");
    explode = true;
    await userEvent.click(screen.getByTestId("studio-kpi-test"));
    expect(await screen.findByText(en.pipelines.test.failed.replace("{status}", "0"))).toBeInTheDocument();
    expect(screen.queryByTestId("studio-kpi-value")).toBeNull();
  });
});
