/**
 * T-3903 (DM-94): the checks on write of a space. Per type and rule the pass rate over 24 hours
 * and 7 days, what was rejected and quarantined, and when; before the Portal has read the
 * gateway's counters it says the checks are unknown, never "all pass". No value on the page.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SpaceChecks } from "../src/pages/spaces/SpaceChecks";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const counts = (pass: number, reject = 0, quarantine = 0) => {
  const all = pass + reject + quarantine;
  return {
    pass,
    reject,
    flag: 0,
    quarantine,
    passRate: all === 0 ? null : pass / all,
  };
};

const row = (
  rule: string,
  day: ReturnType<typeof counts>,
  week: ReturnType<typeof counts>,
  lastRejected: string | null = null,
) => ({
  type: "AirQualityObserved",
  rule,
  attribute: "pm10",
  "24h": day,
  "7d": week,
  lastRejected,
});

const answer = (rules: unknown[]) => ({
  at: "2026-10-12T08:00:00Z",
  windows: { "24h": "2026-10-11T08:00:00Z", "7d": "2026-10-05T09:00:00Z" },
  rules,
});

function renderChecks(body: unknown) {
  return renderPage(<SpaceChecks project="helsinki" space="air" />, {
    path: "/projects/helsinki/spaces/air",
    answer: (url) =>
      url.pathname === "/api/v1/projects/helsinki/spaces/air/quality/checks"
        ? json(body)
        : undefined,
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the checks on write of a space (DM-94)", () => {
  it("says the checks are unknown before a reading, never that all pass", async () => {
    renderChecks({ at: null, windows: null, rules: [] });
    expect(
      await screen.findByText(en.spaces.quality.checks.unknown),
    ).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("says a space without rules declares none", async () => {
    renderChecks(answer([]));
    expect(
      await screen.findByText(en.spaces.quality.checks.noRules),
    ).toBeInTheDocument();
  });

  it("shows every rule passing, and a rule nothing checked yet as unknown", async () => {
    const { container } = renderChecks(
      answer([
        row("no2-plausible", counts(0), counts(0)),
        row("pm10-plausible", counts(120), counts(900)),
      ]),
    );
    const table = await screen.findByRole("table", {
      name: en.spaces.quality.checks.caption,
    });
    const [unchecked, passing] = within(table).getAllByRole("row").slice(1);
    expect(within(unchecked).getAllByText("—").length).toBeGreaterThanOrEqual(
      2,
    );
    expect(within(passing).getAllByText("100%")).toHaveLength(2);
    await expectNoAxeViolations(container);
  });

  it("shows a failing rule's pass rate, rejections and when it last rejected", async () => {
    renderChecks(
      answer([
        row(
          "pm10-plausible",
          counts(90, 10),
          counts(900, 40, 60),
          "2026-10-12T07:00:00Z",
        ),
      ]),
    );
    const table = await screen.findByRole("table", {
      name: en.spaces.quality.checks.caption,
    });
    const failing = within(table).getAllByRole("row")[1];
    expect(within(failing).getAllByText("90%")).toHaveLength(2);
    expect(within(failing).getByText("40")).toBeInTheDocument();
    expect(within(failing).getByText("60")).toBeInTheDocument();
    expect(within(failing).getByText(/2026/)).toBeInTheDocument();
    expect(within(failing).getByText("pm10")).toBeInTheDocument();
  });

  it("names one start when both windows began together, after a restart", async () => {
    renderChecks({
      at: "2026-10-12T08:00:00Z",
      windows: { "24h": "2026-10-12T06:00:00Z", "7d": "2026-10-12T06:00:00Z" },
      rules: [row("range", counts(3), counts(3))],
    });
    expect(await screen.findByText(/^Counted since /)).toBeInTheDocument();
    expect(screen.queryByText(/\(7 days\)/)).toBeNull();
  });
});
