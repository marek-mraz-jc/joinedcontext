import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { RUN_STATES, TERMINAL_STATES } from "./useAgentRun";

/**
 * Where the run stands, as the eight states of Architecture/19 §5 rather than a spinner: a
 * person watching a build wants to know which step is slow, and a run that stopped shows the
 * state it stopped in.
 */
export function RunTimeline({
  status,
  steps,
  tokensUsed,
}: {
  status: string;
  steps: number;
  tokensUsed: number;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const reached = RUN_STATES.indexOf(status as (typeof RUN_STATES)[number]);
  const stopped = TERMINAL_STATES.includes(status) && status !== "published";

  return (
    <section aria-labelledby="run-timeline" className="rounded border border-border p-4">
      <h2 id="run-timeline" className="text-base font-semibold">
        {t("agentRun.timeline.title")}
      </h2>
      <ol className="mt-2 flex flex-wrap gap-x-2 gap-y-1 text-body">
        {RUN_STATES.map((state, index) => {
          const done = reached > index;
          const current = state === status;
          return (
            <li
              key={state}
              aria-current={current ? "step" : undefined}
              className={
                current
                  ? "rounded bg-primary px-2 py-0.5 font-medium text-primary-fg"
                  : done
                    ? "px-2 py-0.5 text-fg"
                    : "px-2 py-0.5 text-fg-muted"
              }
            >
              {t(`agentRun.states.${state}`)}
            </li>
          );
        })}
      </ol>
      {stopped && (
        // An expired run has ended with its preview intact (T-0669): the ordinary tone, not
        // the failure's.
        <p
          role="status"
          className={
            status === "expired"
              ? "mt-2 text-body font-medium text-fg-muted"
              : "mt-2 text-body font-medium text-danger"
          }
        >
          {t(`agentRun.states.${status}`, { defaultValue: status })}
        </p>
      )}
      <p className="mt-2 text-caption text-fg-muted">
        {t("agentRun.timeline.usage", { steps, tokens: tokensUsed.toLocaleString(locale) })}
      </p>
    </section>
  );
}
