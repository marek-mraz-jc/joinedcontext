/**
 * T-4935, PL-105: the KPI editor over several sources writes the v1alpha2 Pipeline jc-core admits
 * from plain choices, checks it as it is typed with each refusal beside its field, previews the
 * value on the sources' pages, and proposes only a draft without problems.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { KpiEditorDialog } from "../src/pages/pipelines/KpiEditor";
import { choiceFormula, emptyDraft, formulaOf, kpiManifest, kpiTargets, leastSources, missingOf, placeOf, withChoice } from "../src/pages/pipelines/kpiEditor";
import type { KpiDraft } from "../src/pages/pipelines/kpiEditor";
import type { Manifest } from "../src/api/manifest";
import { expectNoViolations } from "./checks";
import { sentTo } from "./requests";

const k = en.pipelines.kpiEditor;
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:kpi-writer";

const DRAFT: KpiDraft = {
  ...emptyDraft(TARGET),
  name: "parking-per-stop",
  title: "Occupied places per stop",
  join: "area",
  choice: "per",
  sources: [
    { name: "parking", endpoint: "helsinki-parking", type: "ParkingSite", attribute: "occupied", key: "district" },
    { name: "stops", endpoint: "helsinki-transit", type: "TransitStop", attribute: "", key: "district" },
  ],
};

const endpoint = (name: string, space: string): Manifest => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space },
});

describe("the KPI editor's model", () => {
  it("writes the v1alpha2 Pipeline with each source's attributes and join key", () => {
    expect(kpiManifest("helsinki", DRAFT)).toEqual({
      apiVersion: "joinedcontext.com/v1alpha2",
      kind: "Pipeline",
      metadata: { name: "parking-per-stop", namespace: "helsinki" },
      spec: {
        class: "scheduled",
        schedule: "0 * * * *",
        sources: [
          { name: "parking", endpointRef: { kind: "Endpoint", name: "helsinki-parking" }, query: { type: "ParkingSite", attrs: ["occupied", "district"] } },
          { name: "stops", endpointRef: { kind: "Endpoint", name: "helsinki-transit" }, query: { type: "TransitStop", attrs: ["district"] } },
        ],
        steps: [
          {
            kind: "kpi",
            kpi: {
              name: "parking-per-stop",
              title: "Occupied places per stop",
              formula: "sum(parking.occupied) / count(stops)",
              unitCode: "C62",
              join: { by: "area", keys: { parking: "district", stops: "district" } },
            },
          },
        ],
        outputs: [{ targetEndpoint: TARGET, type: "KeyPerformanceIndicator" }],
      },
    });
  });

  it("writes each plain choice and keeps a typed formula only under Advanced", () => {
    const [a, b] = DRAFT.sources;
    expect(choiceFormula("count", [a, b])).toBe("count(parking)");
    expect(choiceFormula("sum", [a, b])).toBe("sum(parking.occupied)");
    expect(choiceFormula("average", [a, b])).toBe("avg(parking.occupied)");
    expect(choiceFormula("share", [a, { ...b, attribute: "capacity" }])).toBe("sum(parking.occupied) / sum(stops.capacity)");
    const typed = kpiManifest("helsinki", { ...DRAFT, advanced: true, typed: " max(parking.occupied) " });
    expect((typed.spec.steps as { kpi: { formula: string } }[])[0].kpi.formula).toBe("max(parking.occupied)");
    const timed = kpiManifest("helsinki", { ...DRAFT, join: "time" });
    expect((timed.spec.steps as { kpi: { join: unknown } }[])[0].kpi.join).toEqual({ by: "time" });
  });

  it("writes a change as the first sum now minus a week ago, over one source and all its rows (PL-117)", () => {
    const area = { ...DRAFT, join: "area" as const };
    const change = withChoice(area, "change");
    expect(change.sources).toEqual([DRAFT.sources[0]]);
    expect(change.join).toBe("time");
    expect(formulaOf(change)).toBe("sum(parking.occupied) - prev(sum(parking.occupied), P7D)");
    expect(missingOf(change)).toBeUndefined();
    expect(leastSources(change)).toBe(1);
    const spec = kpiManifest("helsinki", change).spec as { sources: { query: { attrs: string[] } }[]; steps: { kpi: { join: unknown } }[] };
    expect(spec.sources).toHaveLength(1);
    expect(spec.sources[0].query.attrs).toEqual(["occupied"]);
    expect(spec.steps[0].kpi.join).toEqual({ by: "time" });

    const back = withChoice(change, "share");
    expect(back.sources).toHaveLength(2);
    expect(leastSources(back)).toBe(2);
    expect(leastSources({ ...back, advanced: true, typed: "prev(sum(parking.occupied), PT1H)" })).toBe(1);
  });

  it("places each refusal beside its field and says what is still missing", () => {
    expect(placeOf("spec.sources[1].endpointRef")).toEqual({ at: "source", index: 1 });
    expect(placeOf("spec.steps.kpi.formula")).toEqual({ at: "formula" });
    expect(placeOf("spec.steps.kpi.unitCode")).toEqual({ at: "unit" });
    expect(placeOf("spec.steps.kpi.join.keys")).toEqual({ at: "join" });
    expect(placeOf("spec")).toEqual({ at: "general" });
    expect(missingOf(emptyDraft(TARGET))).toBe("name");
    expect(missingOf({ ...DRAFT, name: "Bad Name" })).toBe("name");
    expect(missingOf({ ...DRAFT, sources: [DRAFT.sources[0], { ...DRAFT.sources[1], endpoint: "" }] })).toBe("source");
    expect(missingOf({ ...DRAFT, target: "" })).toBe("target");
    expect(missingOf(DRAFT)).toBeUndefined();
  });

  it("offers only the project's -kpi Endpoints as targets", () => {
    const space = (m: Manifest) => m.spec.contextSpaceRef as string;
    expect(kpiTargets([endpoint("helsinki-parking", "parking"), endpoint("kpi-writer", "helsinki-kpi")], space, "hel.fi")).toEqual([
      { name: "kpi-writer", urn: TARGET },
    ]);
  });

  it("offers no target while the organization's domain is unknown (F-UI3-016)", () => {
    const space = (m: Manifest) => m.spec.contextSpaceRef as string;
    expect(kpiTargets([endpoint("kpi-writer", "helsinki-kpi")], space, "")).toEqual([]);
    expect(kpiTargets([endpoint("kpi-writer", "helsinki-kpi")], space, undefined)).toEqual([]);
  });
});

interface Answers {
  problems?: { field: string; reason: string }[];
  readable?: boolean;
  preview?: unknown;
}

function stubFetch(answers: Answers) {
  const json = (body: unknown, status = 200) =>
    Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = input instanceof Request ? input.url : String(input);
    if (url.includes("/kpis/check")) {
      const text = init?.body != null ? String(init.body) : input instanceof Request ? await input.clone().text() : "{}";
      const body = JSON.parse(text || "{}") as { preview?: boolean };
      return json({
        problems: answers.problems ?? [],
        sources: [
          { name: "parking", endpoint: "helsinki-parking", readable: answers.readable ?? true },
          { name: "stops", endpoint: "helsinki-transit", readable: true },
        ],
        ...(body.preview ? { preview: answers.preview } : {}),
      });
    }
    if (url.includes("/endpoints")) {
      return json({
        items: [endpoint("helsinki-parking", "parking"), endpoint("helsinki-transit", "transit"), endpoint("kpi-writer", "helsinki-kpi")],
      });
    }
    if (url.includes("/permissions")) return json({ project: "helsinki", kinds: {} }, 404);
    return json({ items: [], models: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function show(initial: KpiDraft = DRAFT) {
  const onPropose = vi.fn();
  const view = render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <KpiEditorDialog project="helsinki" initial={initial} pending={false} error={null} onClose={() => undefined} onPropose={onPropose} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { onPropose, container: view.container };
}

const propose = () => screen.getByRole("button", { name: en.pipelines.propose });

describe("the KPI editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-1; Secure; path=/";
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("writes the share in use from a plain choice", () => {
    expect(formulaOf({ ...DRAFT, choice: "rest" })).toBe("1 - sum(parking.occupied) / sum(stops.)");
  });

  it("says a preview that stopped at the page is not the final value", async () => {
    stubFetch({
      preview: { values: [{ key: null, name: "parking-per-stop", value: 100 }], rows: { parking: 100, stops: 2 } },
    });
    show();
    await waitFor(() => expect(propose()).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: k.runPreview }));
    expect(await screen.findByText(/stopped at 100 rows of a source/)).toBeInTheDocument();
  });

  it("checks the draft as typed, previews it and proposes the manifest it checked", async () => {
    const fetchMock = stubFetch({
      preview: { values: [{ key: "Kallio", name: "parking-per-stop-kallio", value: 7.5 }], rows: { parking: 3, stops: 2 } },
    });
    const { onPropose } = show();
    await waitFor(() => expect(propose()).toBeEnabled());
    const checked = await sentTo(fetchMock, "/kpis/check")[0].json();
    expect(checked).toMatchObject({ preview: false, pipeline: { apiVersion: "joinedcontext.com/v1alpha2" } });
    await userEvent.click(screen.getByRole("button", { name: k.runPreview }));
    const table = await screen.findByRole("table");
    expect(within(table).getByText("Kallio")).toBeInTheDocument();
    expect(within(table).getByText("7.5")).toBeInTheDocument();
    expect(screen.getByText(/parking: 3 rows/)).toBeInTheDocument();
    await userEvent.click(propose());
    expect(onPropose).toHaveBeenCalledWith(kpiManifest("helsinki", DRAFT));
  });

  it("shows a refusal in words beside its field and keeps Propose shut", async () => {
    stubFetch({ problems: [{ field: "spec.steps.kpi.unitCode", reason: "a ratio of equal units takes C62 or P1, not MIN" }] });
    show({ ...DRAFT, unitCode: "MIN" });
    expect(await screen.findByText(/a ratio of equal units takes C62 or P1/)).toBeInTheDocument();
    expect(screen.getByLabelText(new RegExp(k.unit))).toHaveAttribute("aria-invalid", "true");
    expect(propose()).toBeDisabled();
    expect(screen.getByText(k.fixFirst)).toBeInTheDocument();
  });

  it("says when the pipeline cannot read a source yet and how to ask", async () => {
    stubFetch({ readable: false });
    show();
    expect(await screen.findByText(k.notReadable)).toBeInTheDocument();
    expect(screen.getByText(k.askAccess)).toBeInTheDocument();
  });

  it("offers a change since last week, fixes the join and previews the value with its history", async () => {
    stubFetch({
      preview: { values: [{ key: null, name: "parking-per-stop", value: -6 }], rows: { parking: 48, "parking@P7D": 47 } },
    });
    show({ ...DRAFT, join: "time" });
    await userEvent.selectOptions(screen.getByLabelText(new RegExp(k.choice)), "change");
    expect(screen.queryByTestId("kpi-source-1")).not.toBeInTheDocument();
    expect(screen.getByLabelText(new RegExp(k.join))).toBeDisabled();
    expect(screen.getByText(k.joinHint.change)).toBeInTheDocument();
    expect(screen.getByText(/Reads this source's history/)).toBeInTheDocument();
    expect(screen.queryByText(/prev\(/)).toBeNull();
    await waitFor(() => expect(propose()).toBeEnabled());
    expect(screen.getByText(k.noPreviewYet)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: k.runPreview }));
    const table = await screen.findByRole("table");
    expect(within(table).getByText("-6")).toBeInTheDocument();
    expect(screen.getByText(/parking a week ago: 47 rows/)).toBeInTheDocument();
  });

  it("shows no value and the reason when the source has no history", async () => {
    const reason = "input parking: it has no history 7 days back, so there is no value";
    stubFetch({ preview: { values: [], rows: { parking: 48 }, problem: reason } });
    show(withChoice({ ...DRAFT, join: "time" }, "change"));
    await waitFor(() => expect(propose()).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: k.runPreview }));
    expect(await screen.findByText(reason)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("says what is missing before it checks anything", async () => {
    const fetchMock = stubFetch({});
    show({ ...DRAFT, name: "" });
    expect(screen.getByText(k.missing.name)).toBeInTheDocument();
    expect(propose()).toBeDisabled();
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(sentTo(fetchMock, "/kpis/check")).toHaveLength(0);
  });

  it("has no axe violation", async () => {
    stubFetch({});
    show();
    await waitFor(() => expect(propose()).toBeEnabled());
    await expectNoViolations(document.body);
  });
});
