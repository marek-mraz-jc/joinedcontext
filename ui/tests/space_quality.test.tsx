/**
 * T-2796 (DM-74): the Data quality section of a space. The last daily run in words: the share
 * valid, the failing rules with their example ids, and each pipeline's freshness; before the
 * first run it says so rather than "all valid".
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SpaceQuality } from "../src/pages/spaces/SpaceQuality";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const REPORT = {
  observedAt: "2026-09-25T02:00:00Z",
  checked: 1200,
  invalid: 16,
  truncated: false,
  rules: [
    { rule: "sh:minCount", path: "name", count: 12, examples: ["urn:ngsi-ld:Station:hel.fi:bikes:001"] },
    { rule: "type", path: "", count: 4, examples: [] },
  ],
  freshness: [
    {
      pipeline: "bikes-feed",
      type: "BikeHireDockingStation",
      newest: "2026-09-25T01:58:00Z",
      targetSeconds: 615,
      state: "fresh",
      paused: false,
    },
    { pipeline: "weather", type: "", newest: "2026-09-23T01:00:00Z", targetSeconds: 93600, state: "stale", paused: true },
  ],
};

function renderQuality(answer: () => Response) {
  return renderPage(<SpaceQuality project="helsinki" space="bikes" />, {
    path: "/projects/helsinki/spaces/bikes",
    answer: (url) => (url.pathname === "/api/v1/projects/helsinki/spaces/bikes/quality" ? answer() : undefined),
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the Data quality section (DM-74)", () => {
  it("says the share valid, the rules failing and the freshness in one line (DM-74)", async () => {
    const { container } = renderQuality(() => json(REPORT));
    expect(await screen.findByText("98.7% valid · 2 rules failing · 1 pipeline stale")).toBeInTheDocument();
    const rules = screen.getByRole("table", { name: en.spaces.quality.rulesCaption });
    const [first, second] = within(rules).getAllByRole("row").slice(1);
    expect(within(first).getByText("urn:ngsi-ld:Station:hel.fi:bikes:001")).toBeInTheDocument();
    expect(within(second).getByText(en.spaces.quality.wholeEntity)).toBeInTheDocument();
    // No example ids came: the caller does not read the entities, and the page says why.
    expect(within(second).getByText(en.spaces.quality.examplesHidden)).toBeInTheDocument();
    const freshness = screen.getByRole("table", { name: en.spaces.quality.freshnessCaption });
    const weather = within(freshness).getByRole("row", { name: /weather/ });
    expect(within(weather).getByText(en.spaces.quality.states.stale)).toBeInTheDocument();
    expect(within(weather).getByText(en.spaces.quality.paused)).toBeInTheDocument();
    expect(within(weather).getByText("26 h")).toBeInTheDocument();
    expect(within(freshness).getByText("10 min")).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("before the first run says it has not checked, never that all is valid", async () => {
    renderQuality(() => json({}));
    expect(await screen.findByText(en.spaces.quality.notYet)).toBeInTheDocument();
    expect(screen.queryByText(/valid/)).toBeNull();
  });

  it("an empty space and one without pipelines say so without a freshness claim", async () => {
    renderQuality(() => json({ ...REPORT, checked: 0, invalid: 0, rules: [], freshness: [] }));
    expect(await screen.findByText(`${en.spaces.quality.nothingToCheck} · no rule failing`)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("a read that failed shows the failure and a retry, not an empty report", async () => {
    renderQuality(() =>
      json({ type: "about:blank", title: "Not Found", status: 404, detail: "context space 'bikes' not found" }, 404),
    );
    expect(await screen.findByText(/context space 'bikes' not found/)).toBeInTheDocument();
    expect(screen.queryByText(en.spaces.quality.notYet)).toBeNull();
  });
});
