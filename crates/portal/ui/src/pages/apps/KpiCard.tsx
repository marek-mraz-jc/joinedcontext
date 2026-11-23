import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { readCsrfToken } from "../../api/client";
import { Badge } from "../../components/ui/Badge";
import { Button, buttonClass } from "../../components/ui/Button";
import { Card, Field, InlineError, Input, RadioGroup } from "../../components/ui";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { unitSymbol, unitTitle } from "../../units";

/**
 * An indicator the assistant computed (UI-17, PF-54, PF-55): the value large, its unit, the
 * formula and how many entities it folded, and the one action that matters: writing it into
 * the project's indicator space, through that space's Endpoint, with the person's own session.
 * Nothing is written until the person clicks; the Policy of the indicator space decides.
 * **Keep it updated** asks the assistant for the pipeline that recomputes the indicator on a
 * period or on every change, into the space the person names (AG-74).
 * Every string comes from the run's stream and is drawn as text (AG-46).
 */

/** What the indicator was computed over, as the `compute_kpi` call named it. */
export interface KpiQuery {
  type: string;
  attribute: string;
  agg: string;
  q?: string;
}

export interface Kpi {
  name: string;
  title?: string;
  value: number;
  unit?: string;
  formula: string;
  count: number;
  space: string;
  endpointSlug?: string;
  endpointName?: string;
  entity: Record<string, unknown>;
  query?: KpiQuery;
}

function queryOf(input: unknown): KpiQuery | undefined {
  if (typeof input !== "object" || input === null) {
    return undefined;
  }
  const call = input as Record<string, unknown>;
  if (typeof call.type !== "string" || typeof call.agg !== "string") {
    return undefined;
  }
  return {
    type: call.type,
    attribute: typeof call.attribute === "string" ? call.attribute : "",
    agg: call.agg,
    q: typeof call.q === "string" && call.q !== "" ? call.q : undefined,
  };
}

/**
 * The indicator of a `compute_kpi` step, or none when the payload is not what the Portal wrote;
 * `input`, the step's call, says what it was computed over.
 */
export function kpiOf(output: unknown, input?: unknown): Kpi | null {
  if (typeof output !== "object" || output === null) {
    return null;
  }
  const value = output as Record<string, unknown>;
  if (
    typeof value.name !== "string" ||
    typeof value.value !== "number" ||
    typeof value.formula !== "string" ||
    typeof value.space !== "string" ||
    typeof value.entity !== "object" ||
    value.entity === null
  ) {
    return null;
  }
  return {
    name: value.name,
    title: typeof value.title === "string" ? value.title : undefined,
    value: value.value,
    unit: typeof value.unit === "string" && value.unit !== "" ? value.unit : undefined,
    formula: value.formula,
    count: typeof value.count === "number" ? value.count : 0,
    space: value.space,
    endpointSlug: typeof value.endpointSlug === "string" ? value.endpointSlug : undefined,
    endpointName: typeof value.endpointName === "string" ? value.endpointName : undefined,
    entity: value.entity as Record<string, unknown>,
    query: queryOf(input),
  };
}

/** The message that asks the assistant to keep an indicator updated (AG-74). */
export function keepMessage(
  kpi: Kpi,
  keep: { onChange: boolean; minutes: number; space: string },
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const query = kpi.query;
  // The whole sentence is the person's own message in their own language, and these three
  // fragments were English built in code: a Czech steward pressing "keep it updated" sent the
  // agent a half-Czech instruction and read it back in their transcript (T-1762, UI-11).
  const what = query
    ? query.agg === "count"
      ? t("agentRun.kpi.overCount", { type: query.type })
      : t("agentRun.kpi.overAgg", { agg: query.agg, attribute: query.attribute, type: query.type })
    : kpi.formula;
  const over = query?.q ? t("agentRun.kpi.overWhere", { over: what, filter: query.q }) : what;
  return t(keep.onChange ? "agentRun.kpi.keepOnChangeMessage" : "agentRun.kpi.keepEveryMessage", {
    name: kpi.name,
    over,
    minutes: keep.minutes,
    space: keep.space,
  });
}

/** `18.4`, `1 200`, `0.0031`: enough digits to read, none to mislead. */
export function formatValue(value: number, locale: string): string {
  const digits = Number.isInteger(value) ? 0 : Math.abs(value) >= 100 ? 1 : Math.abs(value) >= 1 ? 2 : 4;
  return new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value);
}

type State = { kind: "idle" } | { kind: "writing" } | { kind: "written" } | { kind: "refused"; detail: string };

export function KpiCard({
  project,
  kpi,
  onSend,
}: {
  project: string;
  kpi: Kpi;
  /** Sends a message into the conversation; without it the card offers no pipeline. */
  onSend?: (text: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [state, setState] = useState<State>({ kind: "idle" });
  const [keeping, setKeeping] = useState(false);
  const [onChange, setOnChange] = useState(false);
  const [minutes, setMinutes] = useState("15");
  const [space, setSpace] = useState(kpi.space);

  const write = async (): Promise<void> => {
    if (!kpi.endpointSlug) {
      return;
    }
    setState({ kind: "writing" });
    try {
      const response = await fetch(`/api/endpoint/${encodeURIComponent(kpi.endpointSlug)}/ngsi-ld/v1/entities`, {
        method: "POST",
        credentials: "same-origin",
        headers: {
          "content-type": "application/ld+json",
          "x-csrf-token": readCsrfToken() ?? "",
          // Every write from the Portal's UI names why (DM-113).
          "JC-Reason": encodeURIComponent(t("agentRun.kpi.reason")),
        },
        body: JSON.stringify(kpi.entity),
      });
      if (response.ok) {
        setState({ kind: "written" });
        return;
      }
      const problem = (await response.json().catch(() => null)) as { detail?: string; title?: string } | null;
      setState({ kind: "refused", detail: problem?.detail ?? problem?.title ?? t("agentRun.kpi.refused", { status: response.status }) });
    } catch {
      setState({ kind: "refused", detail: t("agentRun.kpi.refused", { status: 0 }) });
    }
  };

  return (
    <Card as="section" padding="sm" aria-label={t("agentRun.kpi.title")} className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="accent">{t("apps.kpi.badge")}</Badge>
        <span className="min-w-0 break-words font-medium">{kpi.title ?? kpi.name}</span>
      </div>
      <p className="flex flex-wrap items-baseline gap-2">
        <span className="text-3xl font-semibold tabular-nums" data-testid="kpi-value">
          {formatValue(kpi.value, i18n.language)}
        </span>
        {/* The code list's symbol, the unit's name and code on hover (DM-06, T-2812). */}
        {unitSymbol(kpi.unit) ? (
          <Badge mono title={unitTitle(kpi.unit)}>
            {unitSymbol(kpi.unit)}
          </Badge>
        ) : null}
      </p>
      <dl className="grid gap-1 text-xs sm:grid-cols-[auto_1fr] sm:gap-x-3">
        <dt className="text-fg-muted">{t("agentRun.kpi.formula")}</dt>
        <dd className="break-words font-mono">{kpi.formula}</dd>
        <dt className="text-fg-muted">{t("agentRun.kpi.count")}</dt>
        <dd>{t("agentRun.kpi.entities", { count: kpi.count })}</dd>
        <dt className="text-fg-muted">{t("agentRun.kpi.id")}</dt>
        <dd className="break-all font-mono">{String(kpi.entity.id ?? kpi.name)}</dd>
      </dl>
      <p className="text-xs text-fg-muted">{t("agentRun.kpi.lead", { space: kpi.space })}</p>
      {state.kind === "refused" ? (
        <InlineError>
          {state.detail}
        </InlineError>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        {kpi.endpointSlug ? (
          state.kind === "written" ? (
            <>
              <span className="text-xs text-success">{t("agentRun.kpi.written")}</span>
              <Link
                to="/projects/$project/explore"
                params={{ project }}
                search={{ space: kpi.space, endpoint: kpi.endpointName }}
                className={buttonClass("primary", "sm")}
              >
                {t("agentRun.kpi.view")}
              </Link>
            </>
          ) : (
            <Button
              variant="primary"
              size="sm"
              disabled={state.kind === "writing"}
              disabledReason={t("app.busyReason")}
              onClick={() => {
                void write();
              }}
            >
              {state.kind === "writing" ? t("agentRun.kpi.writing") : t("agentRun.kpi.write")}
            </Button>
          )
        ) : (
          <>
            <span className="text-xs text-fg-muted">{t("agentRun.kpi.noEndpoint", { space: kpi.space })}</span>
            <Link
              to="/projects/$project/$plural"
              params={{ project, plural: "endpoints" }}
              className={buttonClass("secondary", "sm")}
            >
              {t("agentRun.kpi.openEndpoints")}
            </Link>
          </>
        )}
        {onSend && !keeping ? (
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              setKeeping(true);
            }}
          >
            {t("agentRun.kpi.keep")}
          </Button>
        ) : null}
      </div>
      {onSend && keeping ? (
        <form
          aria-label={t("agentRun.kpi.keep")}
          className="flex flex-col gap-2 rounded border border-border p-2 text-xs"
          onSubmit={(event) => {
            event.preventDefault();
            const target = space.trim() === "" ? kpi.space : space.trim();
            const every = Math.max(1, Math.round(Number(minutes)) || 15);
            onSend(keepMessage(kpi, { onChange, minutes: every, space: target }, t));
            setKeeping(false);
          }}
        >
          {/* One question with two answers, so the arrow keys walk them and the tab key steps
              over the pair as one stop; the interval belongs to the first answer and is only
              filled in while that answer is the chosen one. */}
          <RadioGroup
            name={`keep-${kpi.name}`}
            legend={t("agentRun.kpi.keep")}
            value={onChange ? "onChange" : "every"}
            options={[
              { value: "every", label: t("agentRun.kpi.keepEvery") },
              { value: "onChange", label: t("agentRun.kpi.keepOnChange") },
            ]}
            onChange={(choice) => {
              setOnChange(choice === "onChange");
            }}
          />
          <Field id={`keep-${kpi.name}-minutes`} label={t("agentRun.kpi.keepMinutes")}>
            <Input
              id={`keep-${kpi.name}-minutes`}
              type="number"
              min={1}
              value={minutes}
              disabled={onChange}
              title={onChange ? t("agentRun.kpi.minutesUnused") : undefined}
              className="w-24"
              onChange={(event) => {
                setMinutes(event.target.value);
              }}
            />
          </Field>
          <Field id={`keep-${kpi.name}-space`} label={t("agentRun.kpi.keepSpace")}>
            <ResourceNamePicker
              id={`keep-${kpi.name}-space`}
              label={t("agentRun.kpi.keepSpace")}
              labelled
              from={{ project, plural: "spaces" }}
              value={space}
              onChange={setSpace}
              create
            />
          </Field>
          <div>
            <Button type="submit" variant="primary" size="sm">
              {t("agentRun.kpi.keepSend")}
            </Button>
          </div>
        </form>
      ) : null}
    </Card>
  );
}
