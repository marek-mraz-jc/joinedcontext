/**
 * What a run cost (T-3078, AG-97): its model calls with their input, cached and output tokens
 * and the cost the provider reported, read from the run's `usage` frames alone.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { RunCost, runCost } from "../src/pages/apps/RunCost";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

let seq = 0;
function usage(payload: Record<string, unknown>): RunEvent {
  seq += 1;
  return { seq, kind: "usage", payload };
}

const FLASH = "google/gemini-3.8-flash";
const RUN: RunEvent[] = [
  { seq: 0, kind: "status", payload: { status: "building" } },
  usage({ tokensThisStep: 60_030, inputTokens: 60_000, cachedTokens: 0, outputTokens: 30, costUsd: 0.018, model: FLASH }),
  usage({ tokensThisStep: 61_000, inputTokens: 60_990, cachedTokens: 58_000, outputTokens: 10, costUsd: 0.004, model: FLASH }),
  // A provider that reported no cost, and a frame of the editing agent's own estimate.
  usage({ tokensThisStep: 5_020, inputTokens: 5_000, outputTokens: 20, model: "other/model" }),
  usage({ step: 4, tokensThisStep: 900, inputTokens: 900, outputTokens: 0, cumulativeTokens: 127_000 }),
];

describe("runCost", () => {
  it("adds the calls up, the cost only where the provider reported one", () => {
    expect(runCost(RUN)).toEqual({
      calls: 4,
      input: 126_890,
      cached: 58_000,
      output: 60,
      costUsd: 0.022,
      priced: 2,
      models: [FLASH, "other/model"],
    });
  });

  it("is nothing for a run without a model call, and ignores values that are not counts", () => {
    expect(runCost([])).toBeNull();
    expect(runCost([usage({ inputTokens: -5, outputTokens: "7", costUsd: Number.NaN })])).toMatchObject({
      calls: 1,
      input: 0,
      output: 0,
      costUsd: 0,
      priced: 0,
    });
  });
});

describe("RunCost", () => {
  it("names each total and says how much of the cost the provider reported", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <RunCost events={RUN} />
      </I18nextProvider>,
    );
    const panel = screen.getByRole("region", { name: "What the run cost" });
    const value = (term: string) => within(panel).getByText(term).nextElementSibling?.textContent;
    expect(value("Model calls")).toBe("4");
    expect(value("Input tokens")).toBe("126,890");
    expect(value("Read from the cache")).toBe("58,000 (46 %)");
    expect(value("Output tokens")).toBe("60");
    expect(value("Cost")).toBe("$0.022");
    expect(within(panel).getByText("The provider reported the cost of 2 of 4 calls; the others are not in it.")).toBeInTheDocument();
    expect(within(panel).getByText(`Models: ${FLASH}, other/model`)).toBeInTheDocument();
  });

  it("draws nothing for a run with no model call", () => {
    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <RunCost events={[]} />
      </I18nextProvider>,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
