/**
 * T-4817: a pipeline's own page and the list's "Last run" cell. The list said "Runs on its
 * schedule" for 27 of 29 pipelines and nothing about when one last ran or whether it wrote; a
 * name opened the edit form, and the runs lived in a dialog behind "…".
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LastRunCell, nextRunOf } from "../src/pages/pipelines/PipelineOverview";
import { pipelineMatches } from "../src/routes/PipelinesPage";
import type { Manifest } from "../src/api/manifest";
import { InRouter, jsonResponse, list, renderRoute } from "./pageHarness";

const NOW = Date.parse("2026-10-20T12:00:00Z");

const run = (over: Partial<{ sent: number; rejected: number; failed: number; lastAt: string }> = {}) => ({
  run: "2026-10-20T11:00:00Z",
  firstAt: "2026-10-20T11:55:00Z",
  lastAt: "2026-10-20T11:56:00Z",
  sent: 120,
  rejected: 0,
  failed: 0,
  ...over,
});

function cell(props: Partial<Parameters<typeof LastRunCell>[0]>) {
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <LastRunCell project="helsinki" name="air-quality" run={undefined} now={NOW} {...props} />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return screen.findByTestId("pipeline-last-run");
}

describe("the Last run cell", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("says a pipeline that never ran has not run yet", async () => {
    const shown = await cell({ run: null });
    expect(within(shown).getByText(en.pipelines.lastRun.never)).toBeInTheDocument();
  });

  it("says when a good run ended and what it wrote, with no warning", async () => {
    const shown = await cell({ run: run() });
    expect(within(shown).getByText("4 min ago")).toHaveAttribute("dateTime", "2026-10-20T11:56:00Z");
    expect(within(shown).getByText("120 written")).toBeInTheDocument();
    expect(within(shown).queryByText(en.pipelines.lastRun.runFailedBadge)).toBeNull();
    expect(within(shown).queryByRole("link")).toBeNull();
  });

  it("marks a failed run red and links its failures to the runs and its rejections to the rejected list", async () => {
    const shown = await cell({ run: run({ sent: 0, rejected: 2, failed: 7 }) });
    expect(within(shown).getByText(en.pipelines.lastRun.runFailedBadge)).toBeInTheDocument();
    expect(within(shown).getByRole("link", { name: "7 failed" })).toHaveAttribute(
      "href",
      "/projects/helsinki/pipelines/air-quality#runs",
    );
    expect(within(shown).getByRole("link", { name: "2 rejected" })).toHaveAttribute(
      "href",
      "/projects/helsinki/pipelines/air-quality#rejected",
    );
  });

  it("adds a resident runner's totals, saying they count since it started", async () => {
    const shown = await cell({ run: run(), counters: { received: 456667, errors: 20157 } });
    expect(within(shown).getByText(/Since the runner started:/)).toBeInTheDocument();
    expect(within(shown).getByText(/456,667 received/)).toBeInTheDocument();
    expect(within(shown).getByRole("link", { name: "20,157 errors" })).toBeInTheDocument();
  });

  it("says the runs could not be read rather than that it never ran", async () => {
    const shown = await cell({ run: undefined, failed: true });
    expect(within(shown).getByText(en.pipelines.lastRun.unavailable)).toBeInTheDocument();
  });
});

describe("the next run", () => {
  it("is the schedule's next tick, the period after the last run, continuous, or none when paused", () => {
    expect(nextRunOf({ schedule: "0 * * * *" }, true, null, NOW)).toEqual({
      kind: "at",
      at: new Date("2026-10-20T13:00:00Z"),
    });
    expect(nextRunOf({ period: "1h" }, true, run(), NOW)).toEqual({
      kind: "at",
      at: new Date("2026-10-20T12:56:00Z"),
    });
    expect(nextRunOf({ class: "resident" }, true, null, NOW)).toEqual({ kind: "continuous" });
    expect(nextRunOf({ schedule: "0 * * * *" }, false, null, NOW)).toEqual({ kind: "paused" });
  });
});

describe("the list's search and filters", () => {
  const pipeline = (name: string, spec: Record<string, unknown>) =>
    ({ apiVersion: "v", kind: "Pipeline", metadata: { name }, spec }) as unknown as Manifest;
  it("finds by name, source and written type, and filters by state and execution", () => {
    const air = pipeline("air-quality", {
      schedule: "0 * * * *",
      source: { dataSourceRef: { name: "fmi-feed" } },
      output: { type: "AirQualityObserved" },
    });
    expect(pipelineMatches(air, "fmi", "all", "all", "en")).toBe(true);
    expect(pipelineMatches(air, "airquality", "all", "all", "en")).toBe(true);
    expect(pipelineMatches(air, "parking", "all", "all", "en")).toBe(false);
    expect(pipelineMatches(air, "", "paused", "all", "en")).toBe(false);
    expect(pipelineMatches(air, "", "running", "scheduled", "en")).toBe(true);
    expect(pipelineMatches(air, "", "all", "resident", "en")).toBe(false);
  });
});

const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "air-quality", namespace: "helsinki", title: "Air quality" },
  spec: {
    schedule: "0 * * * *",
    source: { dataSourceRef: { name: "fmi-feed" } },
    output: { type: "AirQualityObserved" },
    secretRefs: [{ name: "fmi-credentials", key: "token", value: "hunter2hunter2" }],
  },
  status: { phase: "Live" },
};

const REJECTED = {
  items: [
    {
      id: 9,
      at: "2026-10-20T11:56:00Z",
      rule: "sh:datatype",
      path: "pm10",
      message: "pm10 is not of the slot's datatype",
      record: { pm10: "high" },
    },
  ],
};

function answer(path: string): Response | undefined {
  if (path === "/api/v1/projects/helsinki/pipelines/air-quality") return jsonResponse(PIPELINE);
  if (path.endsWith("/air-quality/runs")) return jsonResponse({ items: [run({ rejected: 1 })] });
  if (path.endsWith("/air-quality/rejected")) return jsonResponse(REJECTED);
  return undefined;
}

const EDITOR_GRANTS = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "editor", binding: "editors", rule: { kinds: ["Pipeline"], verbs: ["read", "propose"] } }],
};
const READER_GRANTS = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "viewer", binding: "viewers", rule: { kinds: ["Pipeline"], verbs: ["read"] } }],
};

describe("a pipeline's own page", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
  });

  it("shows source, schedule, last run, runs and the rejected records, with Pause and Edit for an editor", async () => {
    await renderRoute({ path: "/projects/helsinki/pipelines/air-quality", answer, permissions: EDITOR_GRANTS });
    expect(await screen.findByRole("heading", { level: 1, name: "Air quality" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "fmi-feed" })).toBeInTheDocument();
    expect(screen.getByText("Every hour, at minute 0")).toHaveAttribute("title", "0 * * * *");
    expect(await screen.findByText("pm10 is not of the slot's datatype")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: en.pipelines.overview.runs })).toBeInTheDocument();
    // Names of the secret only, never its value (PL-17).
    expect(screen.getByText("fmi-credentials/token")).toBeInTheDocument();
    expect(screen.queryByText(/hunter2/)).toBeNull();
    await waitFor(() => expect(screen.getByRole("button", { name: en.pipelines.edit })).toBeInTheDocument());
    expect(screen.getByRole("button", { name: en.pipelines.pause })).toBeInTheDocument();
  });

  // UI-44: a control the role lacks stays in place, disabled, and says why; it acts on nothing.
  it("shows a reader the same page with Pause and Edit refused and the reason", async () => {
    const page = await renderRoute({
      path: "/projects/helsinki/pipelines/air-quality",
      answer,
      permissions: READER_GRANTS,
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Air quality" })).toBeInTheDocument();
    expect(await screen.findByText("pm10 is not of the slot's datatype")).toBeInTheDocument();
    for (const name of [en.pipelines.edit, en.pipelines.pause]) {
      await waitFor(() =>
        expect(screen.getByRole("button", { name: new RegExp(name) })).toHaveAttribute("aria-disabled", "true"),
      );
      screen.getByRole("button", { name: new RegExp(name) }).click();
    }
    expect(window.location.pathname).toBe("/projects/helsinki/pipelines/air-quality");
    expect(page.calls().filter((call) => !call.startsWith("GET"))).toEqual([]);
  });

  it("is where a pipeline's name in the list leads", async () => {
    await renderRoute({
      path: "/projects/helsinki/pipelines",
      answer: (path) =>
        path === "/api/v1/projects/helsinki/pipelines" ? jsonResponse(list([PIPELINE])) : answer(path),
    });
    expect(await screen.findByRole("link", { name: "Air quality" })).toHaveAttribute(
      "href",
      "/projects/helsinki/pipelines/air-quality",
    );
  });
});
