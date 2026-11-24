/**
 * The pipelines list's remaining branches, on the list as a page that opens its editor in place
 * (no routed form): search and filters, the three views and their address, the row menu's Runs,
 * Rejected records and Alerts, and proposing a new or edited pipeline.
 *
 * The editors, the data flow map and the health view have tests of their own; they are stubbed
 * here with the few controls that call the page back, so these cases hold what the page does
 * with what they hand it.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { handPrefill } from "../src/assistant/state";
import type { Manifest } from "../src/api/manifest";
import { pipelineMatches, prefilledForm } from "../src/routes/PipelinesPage";
import { json, list, problem, renderPage } from "./page_contract";

const ENVELOPE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "new-one", namespace: "helsinki" },
  spec: { schedule: "0 * * * *" },
};

vi.mock("../src/pages/pipelines/PipelineEditor", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/pages/pipelines/PipelineEditor")>();
  return {
    ...actual,
    PipelineEditorDialog: (props: {
      editing: { metadata: { name: string } } | null;
      initial?: { name?: string };
      error: string | null;
      pending: boolean;
      onOpenChange: (open: boolean) => void;
      onSubmit: (envelope: unknown, draft?: { kind: string; name: string }, files?: Record<string, string>) => void;
      onSeveralSources: () => void;
    }) => (
      <div role="dialog" aria-label="pipeline editor">
        <p>{props.editing ? `editing ${props.editing.metadata.name}` : "creating"}</p>
        <p>{props.initial?.name ? `prefilled ${props.initial.name}` : "not prefilled"}</p>
        {props.error ? <p role="alert">{props.error}</p> : null}
        <button onClick={() => props.onSubmit(ENVELOPE)}>submit</button>
        <button onClick={() => props.onSubmit(ENVELOPE, { kind: "Pipeline", name: "new-one" })}>submit with draft</button>
        <button onClick={() => props.onSubmit(ENVELOPE, undefined, { "bento.yaml": "input: {}" })}>submit with files</button>
        <button onClick={props.onSeveralSources}>several sources</button>
        <button onClick={() => props.onOpenChange(false)}>close editor</button>
      </div>
    ),
  };
});
vi.mock("../src/pages/pipelines/KpiEditor", () => ({
  KpiEditorDialog: (props: { error: string | null; onClose: () => void; onPropose: (envelope: unknown) => void }) => (
    <div role="dialog" aria-label="kpi editor">
      {props.error ? <p role="alert">{props.error}</p> : null}
      <button onClick={() => props.onPropose(ENVELOPE)}>propose kpi</button>
      <button onClick={props.onClose}>close kpi</button>
    </div>
  ),
}));
vi.mock("../src/pages/pipelines/FlowMapView", () => ({
  FlowMapView: (props: { onNewPipeline: (form: unknown) => void }) => (
    <div>
      <p>the data flow map</p>
      <button onClick={() => props.onNewPipeline({ name: "from-map" })}>new from the map</button>
    </div>
  ),
}));
vi.mock("../src/pages/pipelines/PipelineHealth", () => ({
  PipelineHealthView: (props: { onOpenRuns: (name: string) => void }) => (
    <div>
      <p>the source health</p>
      <button onClick={() => props.onOpenRuns("ingest")}>runs of ingest</button>
    </div>
  ),
}));

const { PipelinesPage } = await import("../src/routes/PipelinesPage");

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-00000042", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

const pipeline = (name: string, spec: Record<string, unknown> = {}, status: Record<string, unknown> = { phase: "Live" }, title?: Record<string, string>): Manifest =>
  ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name, namespace: "helsinki", ...(title ? { title } : {}) },
    spec: { class: "resident", ...spec },
    status,
  }) as unknown as Manifest;

const STALLED = { phase: "Live", conditions: [{ type: "StreamWriting", status: "False", message: "nothing landed for 10 minutes" }] };
const ITEMS = [
  pipeline("ingest", { source: { dataSourceRef: { name: "fmi-feed" } }, output: { type: "AirQualityObserved" } }, { phase: "Live" }, { en: "Air ingest" }),
  pipeline("nightly", { class: "scheduled", schedule: "0 3 * * *", enabled: false }, { phase: "Pending" }),
  pipeline("quiet", {}, STALLED),
];

const STEWARD = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read", "propose", "approve"] } }] };
const EDITOR = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read", "propose"] } }] };
const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read"] } }] };

interface Sent {
  method: string;
  path: string;
  dryRun: boolean;
  body: Record<string, unknown>;
}

interface World {
  items?: Manifest[];
  grants?: unknown;
  refuse?: Response;
  listFails?: Response;
}

function show(world: World = {}, path = "/projects/helsinki/pipelines") {
  const sent: Sent[] = [];
  const view = renderPage(<PipelinesPage project="helsinki" />, {
    path,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(world.grants ?? EDITOR);
      if (request.method !== "GET") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        sent.push({ method: request.method, path: url.pathname, dryRun, body: (await request.json()) as Record<string, unknown> });
        if (dryRun) return json({ valid: true, verdict: { ok: true, findings: [] } });
        return world.refuse ?? json(CHANGE, 202);
      }
      if (url.pathname.endsWith("/pipelines")) return world.listFails ?? json(list(world.items ?? ITEMS));
      if (url.pathname.endsWith("/rejected")) {
        return json({
          items: [{ id: 9, at: "2026-10-20T11:56:00Z", run: "2026-10-20T11:00:00Z", rule: "sh:datatype", path: "pm10", message: "pm10 is not of the slot's datatype", record: { pm10: "high" } }],
        });
      }
      if (url.pathname.endsWith("/rejected/groups")) return json({ groups: [], total: 0 });
      if (url.pathname.endsWith("/runs")) {
        return json({ items: [{ run: "2026-10-20T11:00:00Z", firstAt: "2026-10-20T11:55:00Z", lastAt: "2026-10-20T11:56:00Z", sent: 120, rejected: 1, failed: 0 }] });
      }
      return undefined;
    },
  });
  return { ...view, sent };
}

const rowOf = async (name: string) => (await screen.findByText(name)).closest("tr") as HTMLElement;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.sessionStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
});

describe("the list's search and filters", () => {
  const needles = (pipeline: Manifest, needle: string, status: "all" | "running" | "paused" | "notWriting" = "all", klass: "all" | "resident" | "scheduled" = "all") =>
    pipelineMatches(pipeline, needle, status, klass, "en");

  it("keeps a running pipeline for Running and drops a paused one, and the other way round for Paused", () => {
    const [ingest, nightly] = ITEMS;
    expect(needles(ingest, "", "running")).toBe(true);
    expect(needles(nightly, "", "running")).toBe(false);
    expect(needles(ingest, "", "paused")).toBe(false);
    expect(needles(nightly, "", "paused")).toBe(true);
  });

  it("keeps only a pipeline whose stream writes nothing for Not writing", () => {
    const [ingest, , quiet] = ITEMS;
    expect(needles(quiet, "", "notWriting")).toBe(true);
    expect(needles(ingest, "", "notWriting")).toBe(false);
  });

  it("filters by execution class and finds a word in the name, the title, the source or the written type", () => {
    const [ingest, nightly] = ITEMS;
    expect(needles(ingest, "", "all", "scheduled")).toBe(false);
    expect(needles(nightly, "", "all", "scheduled")).toBe(true);
    expect(needles(ingest, "  AIR ingest ")).toBe(true);
    expect(needles(ingest, "fmi-feed")).toBe(true);
    expect(needles(ingest, "airqualityobserved")).toBe(true);
    expect(needles(ingest, "nightly")).toBe(false);
  });

  it("narrows the list with the search box, says how many are shown, and says when nothing matches", async () => {
    show();
    await screen.findByText("quiet");
    const user = userEvent.setup();
    await user.type(screen.getByRole("searchbox", { name: en.pipelines.filter.find }), "nightly");
    await waitFor(() => expect(screen.queryByText("quiet")).toBeNull());
    expect(screen.getByText("nightly")).toBeInTheDocument();
    expect(screen.getByText("1 of 3 pipelines")).toBeInTheDocument();
    await user.clear(screen.getByRole("searchbox", { name: en.pipelines.filter.find }));
    await user.type(screen.getByRole("searchbox", { name: en.pipelines.filter.find }), "zzz-nothing");
    expect(await screen.findByText(en.pipelines.filter.none)).toBeInTheDocument();
  });

  it("filters by status and by execution from the two selects", async () => {
    show();
    await screen.findByText("quiet");
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole("combobox", { name: en.pipelines.filter.status }), "paused");
    await waitFor(() => expect(screen.queryByText("quiet")).toBeNull());
    expect(screen.getByText("nightly")).toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: en.pipelines.filter.status }), "notWriting");
    expect(await screen.findByText("quiet")).toBeInTheDocument();
    expect(screen.queryByText("nightly")).toBeNull();
    await user.selectOptions(screen.getByRole("combobox", { name: en.pipelines.filter.status }), "all");
    await user.selectOptions(screen.getByRole("combobox", { name: en.pipelines.filter.class }), "scheduled");
    await waitFor(() => expect(screen.queryByText("quiet")).toBeNull());
    expect(screen.getByText("nightly")).toBeInTheDocument();
    expect(screen.getByText("1 of 3 pipelines")).toBeInTheDocument();
  });

  it("sorts by the class and by the phase from their headers", async () => {
    show();
    const table = await screen.findByRole("table");
    await within(table).findByText("quiet");
    const names = () =>
      within(table)
        .getAllByRole("row")
        .slice(1)
        .map((row) => within(row).getAllByRole("link")[0].textContent);
    const user = userEvent.setup();
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.class }));
    expect(names()).toEqual(["Air ingest", "quiet", "nightly"]);
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.class }));
    expect(names()).toEqual(["nightly", "Air ingest", "quiet"]);
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.class }));
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.name }));
    expect(names()).toEqual(["Air ingest", "nightly", "quiet"]);
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.name }));
    expect(names()).toEqual(["quiet", "nightly", "Air ingest"]);
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.name }));
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.phase }));
    expect(names()).toEqual(["Air ingest", "quiet", "nightly"]);
    await user.click(within(table).getByRole("button", { name: en.pipelines.field.phase }));
    expect(names()).toEqual(["nightly", "Air ingest", "quiet"]);
  });
});

describe("what a hand-off carries into the editor", () => {
  it("takes nothing, a form as it is, and a Pipeline manifest read back into the form", () => {
    expect(prefilledForm(null)).toBeUndefined();
    expect(prefilledForm({ name: "plain" })).toEqual({ name: "plain" });
    expect(prefilledForm(ENVELOPE as unknown as Record<string, unknown>)?.name).toBe("new-one");
  });

  it("takes nothing from a Pipeline manifest it cannot read", () => {
    expect(prefilledForm({ kind: "Pipeline", metadata: { name: 7 } })).toBeUndefined();
  });
});

describe("the three views", () => {
  it("shows the data flow map for a steward, keeps it in the address, and returns to the list without it", async () => {
    show({ grants: STEWARD });
    await screen.findByText("quiet");
    const user = userEvent.setup();
    await user.click(screen.getByRole("tab", { name: en.pipelines.map.view.map }));
    expect(await screen.findByText("the data flow map")).toBeInTheDocument();
    expect(window.location.search).toContain("view=map");
    expect(screen.queryByText("quiet")).toBeNull();
    await user.click(screen.getByRole("tab", { name: en.pipelines.map.view.list }));
    expect(await screen.findByText("quiet")).toBeInTheDocument();
    expect(window.location.search).not.toContain("view=");
  });

  it("offers the source health view to a steward only, and opens a pipeline's runs from it", async () => {
    show({ grants: STEWARD });
    await screen.findByText("quiet");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("tab", { name: en.pipelines.map.view.health }));
    expect(await screen.findByText("the source health")).toBeInTheDocument();
    expect(window.location.search).toContain("view=health");
    await user.click(screen.getByRole("button", { name: "runs of ingest" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("offers no health view to someone who may not approve", async () => {
    show({ grants: EDITOR });
    await screen.findByText("quiet");
    expect(screen.getByRole("tab", { name: en.pipelines.map.view.map })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: en.pipelines.map.view.health })).toBeNull();
  });

  it("opens on the map when the address says so, and a new pipeline from the map opens the editor prefilled", async () => {
    show({}, "/projects/helsinki/pipelines?view=map");
    expect(await screen.findByText("the data flow map")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "new from the map" }));
    expect(await screen.findByText("creating")).toBeInTheDocument();
    expect(screen.getByText("prefilled from-map")).toBeInTheDocument();
  });

  it("offers neither the map nor the health view when the list cannot be read", async () => {
    show({ listFails: problem(503, "The pipelines are away.") });
    expect(await screen.findByRole("alert")).toHaveTextContent("The pipelines are away.");
    expect(screen.queryByRole("tab")).toBeNull();
  });
});

describe("the row menu", () => {
  async function openMenu(name: string, item: string) {
    const row = await rowOf(name);
    const user = userEvent.setup();
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: item }));
  }

  it("opens the runs and the log of the pipeline, and closes them with Escape", async () => {
    show();
    await openMenu("quiet", en.pipelines.runs.open);
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("opens the rejected records, and leads from one to its run", async () => {
    show();
    await openMenu("quiet", en.pipelines.rejected.open);
    const dialog = await screen.findByRole("dialog");
    await screen.findByText("pm10 is not of the slot's datatype");
    await userEvent.click(await within(dialog).findByRole("button", { name: en.pipelines.rejected.openRun }));
    await waitFor(() => expect(screen.queryByText("pm10 is not of the slot's datatype")).toBeNull());
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("closes the rejected records with Escape", async () => {
    show();
    await openMenu("quiet", en.pipelines.rejected.open);
    await screen.findByRole("dialog");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("opens the alerts of the pipeline and closes them", async () => {
    show();
    await openMenu("quiet", en.alerts.open);
    await screen.findByRole("dialog");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("opens the pipeline's editor on Edit with the pipeline in it, and closes it", async () => {
    show();
    await openMenu("quiet", "Edit");
    expect(await screen.findByText("editing quiet")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "close editor" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "pipeline editor" })).toBeNull());
  });

  it("refuses a reader's Edit in the menu, with the reason", async () => {
    show({ grants: READER });
    const row = await rowOf("quiet");
    const user = userEvent.setup();
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    const edit = await screen.findByRole("menuitem", { name: /^Edit/ });
    await waitFor(() => expect(edit).toHaveAttribute("aria-disabled", "true"));
    await user.click(edit);
    expect(screen.queryByRole("dialog", { name: "pipeline editor" })).toBeNull();
  });

  it("opens a reader's link to a pipeline's editor as the record, read only, and closes it", async () => {
    show({ grants: READER }, "/projects/helsinki/pipelines?edit=quiet");
    const dialog = await screen.findByRole("dialog");
    expect(screen.queryByRole("dialog", { name: "pipeline editor" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
  });
});

describe("proposing from the editor", () => {
  async function newPipeline() {
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.pipelines.add }));
    await screen.findByText("creating");
    return user;
  }

  it("creates the pipeline with a checked POST and shows the change it made", async () => {
    const { sent } = show();
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "submit" }));
    expect(await screen.findByText(/chg-00000042/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "pipeline editor" })).toBeNull();
    const real = sent.filter((one) => !one.dryRun);
    expect(real).toHaveLength(1);
    expect(real[0].method).toBe("POST");
    expect(real[0].path).toBe("/api/v1/projects/helsinki/pipelines");
    expect((real[0].body.metadata as { name: string }).name).toBe("new-one");
    // The editor checked the manifest before it handed it over: the page asks only once.
    expect(sent).toHaveLength(1);
  });

  it("sends the draft the form edited, so the proposal is what was checked", async () => {
    const { sent } = show();
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "submit with draft" }));
    await waitFor(() => expect(sent.some((one) => !one.dryRun)).toBe(true));
    expect(sent.find((one) => !one.dryRun)?.body.draft).toEqual({ kind: "Pipeline", name: "new-one" });
  });

  it("sends the files beside the manifest, an edited bento.yaml", async () => {
    const { sent } = show();
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "submit with files" }));
    await waitFor(() => expect(sent.some((one) => !one.dryRun)).toBe(true));
    expect(sent.find((one) => !one.dryRun)?.body.files).toEqual({ "bento.yaml": "input: {}" });
  });

  it("changes an existing pipeline with a PUT at its own path", async () => {
    const { sent } = show();
    const row = await rowOf("quiet");
    const user = userEvent.setup();
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    await user.click(await screen.findByRole("button", { name: "submit" }));
    await waitFor(() => expect(sent.some((one) => !one.dryRun)).toBe(true));
    const real = sent.find((one) => !one.dryRun) as Sent;
    expect(real.method).toBe("PUT");
    expect(real.path).toBe("/api/v1/projects/helsinki/pipelines/quiet");
  });

  it("keeps the editor open with the server's reason when the proposal is refused", async () => {
    show({ refuse: problem(409, "A change of this pipeline already waits.") });
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "submit" }));
    expect(await screen.findByText("A change of this pipeline already waits.")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "pipeline editor" })).toBeInTheDocument();
  });

  it("hands over to the KPI editor from the recipe, and proposes its manifest unchecked as a new pipeline", async () => {
    const { sent } = show();
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "several sources" }));
    expect(await screen.findByRole("dialog", { name: "kpi editor" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "pipeline editor" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "propose kpi" }));
    expect(await screen.findByText(/chg-00000042/)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "kpi editor" })).toBeNull());
    expect(sent.filter((one) => !one.dryRun).map((one) => one.method)).toEqual(["POST"]);
  });

  it("closes the KPI editor without proposing anything", async () => {
    const { sent } = show();
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "several sources" }));
    await user.click(await screen.findByRole("button", { name: "close kpi" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "kpi editor" })).toBeNull());
    expect(sent).toEqual([]);
  });

  it("closes the editor with nothing sent, and the next New starts empty", async () => {
    const { sent } = show();
    const user = await newPipeline();
    await user.click(screen.getByRole("button", { name: "close editor" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "pipeline editor" })).toBeNull());
    expect(sent).toEqual([]);
    await user.click(screen.getByRole("button", { name: en.pipelines.add }));
    expect(await screen.findByText("not prefilled")).toBeInTheDocument();
  });

  it("offers New from the empty list too", async () => {
    show({ items: [] });
    expect(await screen.findByText(en.pipelines.empty)).toBeInTheDocument();
    const buttons = screen.getAllByRole("button", { name: en.pipelines.add });
    await userEvent.click(buttons[buttons.length - 1]);
    expect(await screen.findByText("creating")).toBeInTheDocument();
  });
});

describe("what the assistant hands over", () => {
  it("opens the editor on the form the assistant filled in", async () => {
    handPrefill("/projects/helsinki/pipelines", { name: "assistant-made" });
    show();
    expect(await screen.findByText("prefilled assistant-made")).toBeInTheDocument();
  });

  it("opens the editor on the pipeline named by ?edit= once the list has it", async () => {
    show({}, "/projects/helsinki/pipelines?edit=nightly");
    expect(await screen.findByText("editing nightly")).toBeInTheDocument();
  });
});

describe("pause and resume", () => {
  it("proposes pausing a running pipeline and resuming a paused one, each as a PUT that keeps the rest", async () => {
    const { sent } = show();
    const user = userEvent.setup();
    const running = await rowOf("quiet");
    await user.click(within(running).getByRole("button", { name: new RegExp(en.pipelines.pause) }));
    await waitFor(() => expect(sent.some((one) => !one.dryRun)).toBe(true));
    const pause = sent.find((one) => !one.dryRun) as Sent;
    expect(pause.method).toBe("PUT");
    expect(pause.path).toBe("/api/v1/projects/helsinki/pipelines/quiet");
    expect((pause.body.spec as { enabled: boolean }).enabled).toBe(false);
    expect(await screen.findByText(/chg-00000042/)).toBeInTheDocument();
    const paused = await rowOf("nightly");
    await user.click(within(paused).getByRole("button", { name: new RegExp(en.pipelines.resume) }));
    await waitFor(() => expect(sent.filter((one) => !one.dryRun)).toHaveLength(2));
    expect((sent.filter((one) => !one.dryRun)[1].body.spec as { enabled: boolean }).enabled).toBe(true);
  });

  it("says why pausing was refused and keeps the list", async () => {
    show({ refuse: problem(403, "You may not pause this pipeline.") });
    const row = await rowOf("quiet");
    await userEvent.click(within(row).getByRole("button", { name: new RegExp(en.pipelines.pause) }));
    expect(await screen.findByRole("alert")).toHaveTextContent("You may not pause this pipeline.");
    expect(screen.getByText("nightly")).toBeInTheDocument();
  });
});
