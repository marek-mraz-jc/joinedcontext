/**
 * T-4844 (DM-104..DM-109): how each pipeline that writes a space turns its writes into history
 * rows, one row per pipeline, type and attribute, with a verdict in words and what to change.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/SpaceTemporalAudit.tsx
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SpaceTemporalAudit } from "../src/pages/spaces/SpaceTemporalAudit";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const URL_PATH = "/api/v1/projects/helsinki/spaces/helsinki/temporal-audit";
const T = en.spaces.temporal;

const row = (over: Record<string, unknown>) => ({
  pipeline: "hsl-hfp-vehicles",
  entityType: "Vehicle",
  attribute: "speed",
  record: "every",
  observedAt: ["mapped"],
  unitCode: ["MTS"],
  datasetId: [],
  history: "PT1H",
  lifetime: ["expiresAt in mapping"],
  verdict: "ok",
  ...over,
});

const AUDIT = {
  rows: [
    row({}),
    row({ attribute: "transportMode", unitCode: [], observedAt: ["received"], verdict: "duplicates-rows" }),
    row({ pipeline: "air-quality", entityType: "AirQualityObserved", attribute: "pm10", history: "P30D", lifetime: [] }),
  ],
  unread: [],
  verdicts: { ok: 2, "loses-history": 0, "unbounded-growth": 0, "duplicates-rows": 1, "stamped-on-receive": 0 },
};

function renderAudit(answer: () => Response | undefined) {
  return renderPage(<SpaceTemporalAudit project="helsinki" space="helsinki" />, {
    path: "/projects/helsinki/spaces/helsinki",
    answer: (url) => (url.pathname === URL_PATH ? answer() : undefined),
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the temporal audit of a space (T-4844)", () => {
  it("shows each attribute's recording, time source, unit, keeping and verdict in words", async () => {
    const { container } = renderAudit(() => json(AUDIT));
    const table = await screen.findByRole("table", { name: T.caption });
    const speed = within(table).getByRole("row", { name: /speed/ });
    expect(within(speed).getByText(T.recordMode.every)).toBeInTheDocument();
    expect(within(speed).getByText(`Time: ${T.source.mapped}`)).toBeInTheDocument();
    expect(within(speed).getByText("Unit: MTS")).toBeInTheDocument();
    expect(within(speed).getByText("History 1 hour")).toBeInTheDocument();
    expect(within(speed).getByText(`Entity ends: ${T.expiresMapped}`)).toBeInTheDocument();
    expect(within(speed).getByText(T.verdict.ok)).toBeInTheDocument();
    const mode = within(table).getByRole("row", { name: /transportMode/ });
    expect(within(mode).getByText(`Time: ${T.source.received}`)).toBeInTheDocument();
    expect(within(mode).getByText(T.verdict["duplicates-rows"])).toBeInTheDocument();
    expect(within(mode).getByText(T.fix["duplicates-rows"])).toBeInTheDocument();
    expect(screen.getByText(`${T.verdict["duplicates-rows"]}: 1`)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("says why a value without a unit stamped on receipt still matches its readings (T-6089)", async () => {
    renderAudit(() =>
      json({
        ...AUDIT,
        rows: [
          row({ attribute: "dateObserved", record: "onChange", unitCode: [], observedAt: ["received"] }),
          row({ attribute: "pm10", record: "onChange", observedAt: ["received"], verdict: "stamped-on-receive" }),
          row({ attribute: "name", record: "none", unitCode: [], observedAt: ["received"], history: "off" }),
        ],
      }),
    );
    const table = await screen.findByRole("table", { name: T.caption });
    const date = within(table).getByRole("row", { name: /dateObserved/ });
    expect(within(date).getByText(`Time: ${T.source.received}`)).toBeInTheDocument();
    expect(within(date).getByText(T.verdict.ok)).toBeInTheDocument();
    expect(within(date).getByText(T.receivedOk)).toBeInTheDocument();
    // The summary and the row name the same thing: a measurement stamped on receipt.
    const pm10 = within(table).getByRole("row", { name: /pm10/ });
    expect(within(pm10).getByText("Measurement stamped on receipt")).toBeInTheDocument();
    expect(within(pm10).queryByText(T.receivedOk)).not.toBeInTheDocument();
    // Without history there are no rows to explain.
    const name = within(table).getByRole("row", { name: /^name/ });
    expect(within(name).queryByText(T.receivedOk)).not.toBeInTheDocument();
  });

  it("filters by pipeline and by verdict, and says when nothing matches", async () => {
    renderAudit(() => json(AUDIT));
    const table = await screen.findByRole("table", { name: T.caption });
    expect(within(table).getAllByRole("row")).toHaveLength(4);
    await userEvent.selectOptions(screen.getByLabelText(T.pipelineFilter), "air-quality");
    expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(2);
    await userEvent.selectOptions(screen.getByLabelText(T.verdictFilter), "duplicates-rows");
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.getByText(T.noMatch)).toBeInTheDocument();
  });

  it("says so when no readable pipeline writes the space", async () => {
    renderAudit(() => json({ rows: [], unread: [], verdicts: {} }));
    expect(await screen.findByText(T.empty)).toBeInTheDocument();
  });

  it("names the pipelines whose mapping the forge did not answer", async () => {
    renderAudit(() => json({ ...AUDIT, unread: ["linked-events"] }));
    expect(await screen.findByText(/linked-events could not be read/)).toBeInTheDocument();
  });

  it("shows the server's reason when the audit fails", async () => {
    renderAudit(() =>
      json({ type: "about:blank", title: "Not Found", status: 404, detail: "context space 'helsinki' not found" }, 404),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("context space 'helsinki' not found");
  });
});
