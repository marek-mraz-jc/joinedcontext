/**
 * A pipeline's schedule in words (T-3259): how often, picked as a person says it, written as the
 * five cron fields the runner reads in UTC, the cron editable for whoever knows it, the next five
 * starts in the person's own time zone, and a warning when the schedule asks a source more often
 * than its catalogue record says it changes.
 */
import { useContext, useState } from "react";
import type { JSX } from "react";
import { ariaDescribedByIds } from "@rjsf/utils";
import type { WidgetProps } from "@rjsf/utils";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../../api/client";
import { asManifests, refName } from "../../../api/manifest";
import type { Manifest } from "../../../api/manifest";
import { Alert, InlineError, Input, Select } from "../../ui";
import { useShownErrors } from "../touched";
import { FormDataContext } from "./EntitySelectorField";
import { FormProjectContext } from "./ModelWidgets";
import {
  FREQUENCY_SECONDS,
  HOUR_STEPS,
  MINUTE_STEPS,
  choiceOf,
  cronOf,
  intervalSeconds,
  nextRuns,
  parseCron,
} from "../../../pages/pipelines/cron";
import type { Choice } from "../../../pages/pipelines/cron";

const KINDS: Choice["kind"][] = ["minutes", "hours", "hourly", "daily", "weekly", "custom"];
const WEEKDAYS = [1, 2, 3, 4, 5, 6, 0];

/** The choice of `kind` that keeps what the current one already says (the time, the minute). */
function switched(kind: Choice["kind"], from: Choice): Choice {
  const hour = "hour" in from ? from.hour : 3;
  const minute = "minute" in from ? from.minute : 0;
  switch (kind) {
    case "minutes":
      return { kind, every: 15 };
    case "hours":
      return { kind, every: 6, minute };
    case "hourly":
      return { kind, minute };
    case "daily":
      return { kind, hour, minute };
    case "weekly":
      return { kind, weekday: 1, hour, minute };
    case "custom":
      return { kind };
  }
}

const two = (n: number) => String(n).padStart(2, "0");

/** `HH:MM` as a time field gives it, or `undefined` when it is not one. */
function timeOf(text: string): { hour: number; minute: number } | undefined {
  const match = /^(\d{2}):(\d{2})$/.exec(text);
  return match ? { hour: Number(match[1]), minute: Number(match[2]) } : undefined;
}

/** The catalogue frequency of the Endpoint a pipeline reads, when its source is one. */
function useSourceFrequency(): string | undefined {
  const project = useContext(FormProjectContext);
  const form = useContext(FormDataContext) as { source?: { endpointRef?: unknown } } | undefined;
  const source = refName(form?.source?.endpointRef);
  const endpoints = useQuery({
    queryKey: queryKeys.list(project ?? "", "endpoints"),
    enabled: Boolean(project && source),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: project ?? "", plural: "endpoints" } },
        }),
      ),
  });
  const endpoint = asManifests((endpoints.data as { items?: Manifest[] } | undefined)?.items ?? []).find(
    (one) => one.metadata.name === source,
  );
  const frequency = (endpoint?.spec as { catalog?: { frequency?: unknown } } | undefined)?.catalog?.frequency;
  return typeof frequency === "string" ? frequency : undefined;
}

export function CronScheduleWidget(props: WidgetProps): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, onBlur, rawErrors } = props;
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const errors = useShownErrors(id, rawErrors);
  const text = typeof value === "string" ? value : "";
  const read = text === "" ? ({ kind: "daily", hour: 3, minute: 0 } as Choice) : choiceOf(text);
  // "Custom" holds while the person writes the cron themselves, whatever the text still says.
  const [custom, setCustom] = useState(false);
  const choice: Choice = custom ? { kind: "custom" } : read;
  const cron = parseCron(text);
  const frequency = useSourceFrequency();
  const off = disabled || readonly;

  const write = (next: Choice) => {
    const written = cronOf(next);
    if (written !== undefined) onChange(written);
  };
  const runs = cron ? nextRuns(cron, new Date()) : [];
  const local = new Intl.DateTimeFormat(locale, {
    weekday: "short",
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    timeZoneName: "short",
  });
  const every = cron ? intervalSeconds(cron, new Date()) : undefined;
  const sourceEvery = frequency ? FREQUENCY_SECONDS[frequency] : undefined;
  const tooOften = every !== undefined && sourceEvery !== undefined && every < sourceEvery;
  const timeValue = "hour" in choice ? `${two(choice.hour)}:${two(choice.minute)}` : "";

  return (
    <div className="flex flex-col gap-2" data-testid="cron-schedule">
      <div className="flex flex-wrap items-end gap-2">
        <label className="flex flex-col gap-1 text-caption text-fg-muted">
          {t("pipelines.schedule.how")}
          <Select
            aria-describedby={ariaDescribedByIds(id)}
            value={choice.kind}
            disabled={off}
            onChange={(event) => {
              const next = switched(event.target.value as Choice["kind"], read);
              setCustom(next.kind === "custom");
              if (next.kind !== "custom") write(next);
              else if (text === "") onChange(cronOf(read));
            }}
          >
            {KINDS.map((kind) => (
              <option key={kind} value={kind}>
                {t(`pipelines.schedule.kind.${kind}`)}
              </option>
            ))}
          </Select>
        </label>
        {choice.kind === "minutes" || choice.kind === "hours" ? (
          <label className="flex flex-col gap-1 text-caption text-fg-muted">
            {t(`pipelines.schedule.every.${choice.kind}`)}
            <Select
              aria-describedby={ariaDescribedByIds(id)}
              value={String(choice.every)}
              disabled={off}
              onChange={(event) => write({ ...choice, every: Number(event.target.value) })}
            >
              {(choice.kind === "minutes" ? MINUTE_STEPS : HOUR_STEPS).map((step) => (
                <option key={step} value={step}>
                  {t(`pipelines.schedule.step.${choice.kind}`, { count: step })}
                </option>
              ))}
            </Select>
          </label>
        ) : null}
        {choice.kind === "hourly" || choice.kind === "hours" ? (
          <label className="flex flex-col gap-1 text-caption text-fg-muted">
            {t("pipelines.schedule.atMinute")}
            <Input
              aria-describedby={ariaDescribedByIds(id)}
              type="number"
              min={0}
              max={59}
              className="w-20"
              value={choice.minute}
              disabled={off}
              onChange={(event) => {
                const minute = Number(event.target.value);
                if (Number.isInteger(minute) && minute >= 0 && minute <= 59) write({ ...choice, minute });
              }}
            />
          </label>
        ) : null}
        {choice.kind === "weekly" ? (
          <label className="flex flex-col gap-1 text-caption text-fg-muted">
            {t("pipelines.schedule.on")}
            <Select
              aria-describedby={ariaDescribedByIds(id)}
              value={String(choice.weekday)}
              disabled={off}
              onChange={(event) => write({ ...choice, weekday: Number(event.target.value) })}
            >
              {WEEKDAYS.map((day) => (
                <option key={day} value={day}>
                  {new Intl.DateTimeFormat(locale, { weekday: "long", timeZone: "UTC" }).format(
                    new Date(Date.UTC(2026, 9, 4 + day)),
                  )}
                </option>
              ))}
            </Select>
          </label>
        ) : null}
        {choice.kind === "daily" || choice.kind === "weekly" ? (
          <label className="flex flex-col gap-1 text-caption text-fg-muted">
            {t("pipelines.schedule.atTime")}
            <Input
              aria-describedby={ariaDescribedByIds(id)}
              type="time"
              className="w-32"
              value={timeValue}
              disabled={off}
              onChange={(event) => {
                const at = timeOf(event.target.value);
                if (at) write({ ...choice, ...at });
              }}
            />
          </label>
        ) : null}
      </div>
      <label className="flex flex-col gap-1 text-caption text-fg-muted">
        {t("pipelines.schedule.cron")}
        <Input
          id={id}
          className="font-mono"
          value={text}
          required={required}
          disabled={off}
          spellCheck={false}
          autoComplete="off"
          aria-describedby={ariaDescribedByIds(id)}
          aria-invalid={(errors && errors.length > 0) || (text !== "" && !cron) ? "true" : undefined}
          onChange={(event) => onChange(event.target.value === "" ? undefined : event.target.value)}
          onBlur={(event) => onBlur?.(id, event.target.value)}
        />
      </label>
      {text !== "" && !cron ? (
        <InlineError>
          {t("pipelines.schedule.invalid")}
        </InlineError>
      ) : null}
      {runs.length > 0 ? (
        <div className="flex flex-col gap-1">
          <span className="text-caption font-medium text-fg">{t("pipelines.schedule.next")}</span>
          <ol className="flex flex-col text-caption text-fg-muted" aria-label={t("pipelines.schedule.next")}>
            {runs.map((run) => (
              <li key={run.getTime()}>
                <time dateTime={run.toISOString()}>{local.format(run)}</time>
              </li>
            ))}
          </ol>
        </div>
      ) : cron ? (
        <p className="text-caption text-fg-muted">{t("pipelines.schedule.never")}</p>
      ) : null}
      {tooOften ? (
        <Alert tone="warning" role="status">
          {t("pipelines.schedule.tooOften", { frequency: t(`choice.frequency.${frequency}`, frequency ?? "") })}
        </Alert>
      ) : null}
    </div>
  );
}
