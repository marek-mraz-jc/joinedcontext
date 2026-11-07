import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { RunEvent } from "./useAgentRun";

/** What a run's model calls added up to (T-3078). */
export interface RunCostTotals {
  calls: number;
  input: number;
  cached: number;
  output: number;
  /** The sum of the costs the provider reported, in USD. */
  costUsd: number;
  /** How many calls carried a reported cost. */
  priced: number;
  /** Every model named, in the order first seen. */
  models: string[];
}

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0;
}

/**
 * The run's model calls added up from its `usage` frames alone (T-3078, AG-97): each frame is one
 * call (T-3072), with the input, the part of it read from the provider's cache, the output and,
 * when the provider reports it, the cost. `null` for a run that made no call.
 */
export function runCost(events: RunEvent[]): RunCostTotals | null {
  const totals: RunCostTotals = { calls: 0, input: 0, cached: 0, output: 0, costUsd: 0, priced: 0, models: [] };
  for (const event of events) {
    if (event.kind !== "usage") {
      continue;
    }
    const { inputTokens, cachedTokens, outputTokens, costUsd, model } = event.payload;
    totals.calls += 1;
    totals.input += count(inputTokens);
    totals.cached += count(cachedTokens);
    totals.output += count(outputTokens);
    if (typeof costUsd === "number" && Number.isFinite(costUsd) && costUsd >= 0) {
      totals.costUsd += costUsd;
      totals.priced += 1;
    }
    if (typeof model === "string" && model !== "" && !totals.models.includes(model)) {
      totals.models.push(model);
    }
  }
  if (totals.calls === 0) {
    return null;
  }
  // Sums of small decimals drift; a millionth of a dollar is below anything shown.
  totals.costUsd = Math.round(totals.costUsd * 1e6) / 1e6;
  return totals;
}

/**
 * What the run cost, beside where its time went (T-3078): the calls, the tokens in and out, how
 * much of the input the provider's cache served, and the cost the provider reported. A call
 * without a reported cost is said, never priced by a guess. Nothing is drawn for a run that made
 * no model call.
 */
export function RunCost({ events }: { events: RunEvent[] }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const totals = runCost(events);
  if (totals === null) {
    return null;
  }
  const number = new Intl.NumberFormat(i18n.language);
  const money = new Intl.NumberFormat(i18n.language, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  });
  const share = totals.input > 0 ? Math.round((totals.cached / totals.input) * 100) : 0;
  const rows: [string, string][] = [
    ["calls", number.format(totals.calls)],
    ["input", number.format(totals.input)],
    ["cached", t("agentRun.cost.cachedValue", { tokens: number.format(totals.cached), share })],
    ["output", number.format(totals.output)],
    ["cost", totals.priced > 0 ? money.format(totals.costUsd) : t("agentRun.cost.unpriced")],
  ];
  return (
    <section aria-labelledby="run-cost" className="rounded border border-border p-4">
      <h2 id="run-cost" className="text-base font-semibold">
        {t("agentRun.cost.title")}
      </h2>
      <dl className="mt-2 flex flex-wrap gap-x-6 gap-y-1 text-body">
        {rows.map(([part, value]) => (
          <div key={part} className="flex gap-2">
            <dt className="text-fg-muted">{t(`agentRun.cost.${part}`)}</dt>
            <dd className="font-medium tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
      {totals.priced < totals.calls && totals.priced > 0 && (
        <p className="mt-2 text-caption text-fg-muted">
          {t("agentRun.cost.partlyPriced", { priced: totals.priced, calls: totals.calls })}
        </p>
      )}
      {totals.models.length > 0 && (
        <p className="mt-1 text-caption text-fg-muted">{t("agentRun.cost.models", { models: totals.models.join(", ") })}</p>
      )}
    </section>
  );
}
