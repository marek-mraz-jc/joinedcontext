/**
 * T-4892: "Source vs space" on a snapshot pipeline's page. The counts of the last complete run
 * beside the space, the orphans and the records missing from the space, a held run a steward
 * releases (orphans 3 -> 0, the three deleted with their reason), the CSV, a reader who may not
 * decide, ids left out for a caller who may not read the space, and the space's own tombstones.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SourceRemovalsPanel, sourceCsv } from "../src/pages/pipelines/PipelineSourceReport";
import type { components } from "../src/api/schema";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

type Report = components["schemas"]["PipelineSourceDeletes"];

const PAGE = "/projects/helsinki/pipelines/registry";
const REPORT = "/api/v1/projects/helsinki/pipelines/registry/source-deletes";
const S = en.pipelines.source;
const id = (n: number) => `urn:ngsi-ld:Dataset:hel.fi:open:ds-${n}`;

const pipeline = (mode: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "registry", namespace: "helsinki" },
  spec: {
    class: "scheduled",
    schedule: "0 3 * * *",
    source: { dataSourceRef: { name: "ckan" } },
    sourceDeletes: { mode, misses: 2, maxShare: 5 },
  },
  status: { phase: "Live" },
});

const removal = (n: number) => ({
  id: id(n),
  pipeline: "registry",
  run: "r3",
  at: "2026-10-21T03:05:00Z",
  reason: "source-snapshot: pipeline registry run r3",
});

const held: Report = {
  misses: 2,
  maxShare: 5,
  ledger: 50,
  next: [],
  due: [id(17), id(33), id(4)],
  hold: { run: "r3", reason: "share-exceeded", would: 3, ledger: 50 },
  space: "open",
  lastComplete: { run: "r3", at: "2026-10-21T03:00:00Z", read: 48 },
  orphans: 3,
  orphanIds: [id(17), id(33), id(4)],
  missing: 1,
  missingIds: ["urn:ngsi-ld:Dataset:hel.fi:open:bad"],
  removed: [],
  idsHidden: false,
};

const released: Report = {
  ...held,
  ledger: 47,
  due: [],
  hold: null,
  orphans: 0,
  orphanIds: [],
  removed: [removal(17), removal(33), removal(4)],
};

const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "r", binding: "b", rule: { kinds: ["Pipeline"], verbs } }],
});

function world(mode: string, first: Report | Response) {
  const seen = { decided: [] as unknown[] };
  let current: Report | Response = first;
  const answer = (path: string, request: Request): Response | undefined => {
    if (path === "/api/v1/projects/helsinki/pipelines/registry") return jsonResponse(pipeline(mode));
    if (path === REPORT && request.method === "POST") {
      void request
        .clone()
        .json()
        .then((body: unknown) => seen.decided.push(body));
      current = released;
      return jsonResponse({ deleting: 3 }, 202);
    }
    if (path === REPORT) return current instanceof Response ? current.clone() : jsonResponse(current);
    return undefined;
  };
  return { seen, answer };
}

describe("a snapshot pipeline's source vs space", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.history.pushState({}, "", "/");
  });

  it("shows the counts, the held run, and after a release goes from 3 orphans to 0 with the reason", async () => {
    const { seen, answer } = world("snapshot", held);
    const page = await renderRoute({ path: PAGE, answer, permissions: grants(["read", "approve"]) });
    const panel = await screen.findByTestId("source-report");
    expect(within(panel).getByText(S.atSource).nextSibling).toHaveTextContent("48");
    expect(within(panel).getByText(S.inSpace).nextSibling).toHaveTextContent("50");
    expect(within(panel).getByText(S.orphans).nextSibling).toHaveTextContent("3");
    expect(within(panel).getByText(S.missing).nextSibling).toHaveTextContent("1");
    expect(within(panel).getByText("Run r3 deleted nothing")).toBeInTheDocument();
    expect(within(panel).getByText(S.noneRemoved.replace("{misses}", "2"))).toBeInTheDocument();
    expect(within(screen.getByTestId("source-orphans")).getByText(id(33))).toBeInTheDocument();
    await expectAxeClean(panel);

    await userEvent.click(within(panel).getByRole("button", { name: "Delete 3 anyway" }));
    await waitFor(() => expect(seen.decided).toEqual([{ decision: "delete" }]));
    await waitFor(() => expect(within(panel).getByText(S.orphans).nextSibling).toHaveTextContent("0"));
    expect(within(panel).queryByText("Run r3 deleted nothing")).toBeNull();
    const rows = within(panel).getAllByText("Removed at the source (pipeline registry, run r3)");
    expect(rows).toHaveLength(3);
    expect(within(panel).getByText(id(4))).toBeInTheDocument();
    expect(page.calls()).toContain(`POST ${REPORT}`);
  });

  it("lets a reader see the report but not release or keep the held run", async () => {
    const { answer } = world("snapshot", held);
    await renderRoute({ path: PAGE, answer, permissions: grants(["read"]) });
    const panel = await screen.findByTestId("source-report");
    for (const name of ["Delete 3 anyway", S.keep]) {
      expect(within(panel).getByRole("button", { name })).toHaveAttribute("aria-disabled", "true");
    }
  });

  it("counts without ids for a caller who may not read the space, and offers no CSV", async () => {
    const { answer } = world("snapshot", { ...held, hold: null, orphanIds: [], missingIds: [], due: [], idsHidden: true });
    await renderRoute({ path: PAGE, answer, permissions: grants(["read"]) });
    const panel = await screen.findByTestId("source-report");
    expect(within(panel).getByRole("status")).toHaveTextContent(S.idsHidden.replace("{space}", "open"));
    expect(screen.queryByTestId("source-orphans")).toBeNull();
    expect(within(panel).getByRole("button", { name: S.export })).toHaveAttribute("aria-disabled", "true");
  });

  it("says what to set when the pipeline keeps what its source drops, and asks nothing", async () => {
    const { answer } = world("never", held);
    const page = await renderRoute({ path: PAGE, answer, permissions: grants(["read"]) });
    expect(await screen.findByText(S.notSnapshot)).toBeInTheDocument();
    expect(page.calls()).not.toContain(`GET ${REPORT}`);
  });

  it("names the failure a person can act on when the report cannot be read", async () => {
    const { answer } = world("snapshot", problem(503, "the pipeline's source ledger could not be read; try again"));
    await renderRoute({ path: PAGE, answer, permissions: grants(["read"]) });
    expect(await screen.findByText("the pipeline's source ledger could not be read; try again")).toBeInTheDocument();
  });
});

describe("the report's CSV", () => {
  it("has one row per id, its side and for a removal the run, time and reason, quoted where needed", () => {
    const csv = sourceCsv({ ...released, orphanIds: [id(1)], removed: [{ ...removal(4), reason: 'a, "b"' }] });
    expect(csv.split("\r\n")).toEqual([
      "side,id,run,at,reason",
      `orphan,${id(1)},,,`,
      "missing,urn:ngsi-ld:Dataset:hel.fi:open:bad,r3,,",
      `removed,${id(4)},r3,2026-10-21T03:05:00Z,"a, ""b"""`,
      "",
    ]);
  });

  it("writes an id a spreadsheet would run as a formula as text (F-UI3-009)", () => {
    const csv = sourceCsv({ ...released, orphanIds: ['=HYPERLINK("https://evil.example","x")', "@SUM(1)"], missingIds: ["-1+2"], removed: [] });
    expect(csv.split("\r\n").slice(1, 4)).toEqual([
      `orphan,"'=HYPERLINK(""https://evil.example"",""x"")",,,`,
      "orphan,'@SUM(1),,,",
      "missing,'-1+2,r3,,",
    ]);
  });
});

describe("the space's tombstones from source deletes", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("lists each removal as removed at the source with its pipeline and run, and nothing when empty", async () => {
    const answers = [[removal(4)], []];
    for (const body of answers) {
      vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(jsonResponse(body))));
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      const { unmount } = render(
        <QueryClientProvider client={client}>
          <I18nextProvider i18n={i18n}>
            <SourceRemovalsPanel project="helsinki" space="open" />
          </I18nextProvider>
        </QueryClientProvider>,
      );
      if (body.length > 0) {
        const panel = await screen.findByTestId("source-removals");
        expect(within(panel).getByText(id(4))).toBeInTheDocument();
        expect(within(panel).getByText(/Removed at the source \(pipeline registry, run r3\)/)).toBeInTheDocument();
      } else {
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(screen.queryByTestId("source-removals")).toBeNull();
      }
      unmount();
    }
  });
});
