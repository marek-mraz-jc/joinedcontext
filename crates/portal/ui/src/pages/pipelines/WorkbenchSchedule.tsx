/**
 * The workbench's "When it runs" step (G-23, PL-69): the schedule in words with the next five
 * starts and the source-frequency warning (the form's own cron widget), a pipeline that runs as
 * records arrive said as such, and the project's shortest allowed clock (PL-116) checked at the
 * field with its reason, before the dry run refuses it.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { projectDetailQuery } from "../../api/projects";
import { CronScheduleWidget } from "../../components/forms/widgets/CronSchedule";
import { FormDataContext } from "../../components/forms/widgets/EntitySelectorField";
import { FormProjectContext } from "../../components/forms/widgets/ModelWidgets";
import { Alert, Button, Field } from "../../components/ui";
import { intervalSeconds, parseCron } from "./cron";
import type { PipelineForm } from "./PipelineEditor";

/** What a new schedule starts as: every day at 03:00 UTC, the quiet hour of most sources. */
export const FIRST_SCHEDULE = "0 3 * * *";

/** The draft with `schedule` set: a schedule is what a scheduled pipeline runs on (PL-04). */
export function scheduled(draft: PipelineForm | undefined, schedule: string | undefined): PipelineForm {
  const next: PipelineForm = { ...(draft ?? {}), schedule };
  if (schedule !== undefined && (draft?.class === undefined || draft.class === "auto")) next.class = "scheduled";
  return next;
}

/** The shortest gap between two starts of `schedule`, or `undefined` when it is no cron. */
export function scheduleGap(schedule: string | undefined, from: Date = new Date()): number | undefined {
  const cron = schedule ? parseCron(schedule) : undefined;
  return cron ? intervalSeconds(cron, from) : undefined;
}

export interface WorkbenchScheduleProps {
  project: string;
  draft: PipelineForm | undefined;
  onChange: (form: PipelineForm) => void;
}

export function WorkbenchSchedule({ project, draft, onChange }: WorkbenchScheduleProps): JSX.Element {
  const { t } = useTranslation();
  const detail = useQuery({ ...projectDetailQuery(project), enabled: project !== "" });
  const floor = (detail.data as { spec?: { quotas?: { minPipelinePeriodSeconds?: unknown } } } | undefined)?.spec?.quotas
    ?.minPipelinePeriodSeconds;
  const gap = scheduleGap(draft?.schedule);
  const below = typeof floor === "number" && gap !== undefined && gap < floor;
  // Open once the person asks for a schedule, so clearing the cron to type another keeps the field.
  const [open, setOpen] = useState(false);

  if (draft?.class === "resident") {
    return <p className="text-caption text-fg-muted">{t("pipelines.workbench.schedule.resident")}</p>;
  }
  if (!draft?.schedule && !open) {
    return (
      <div className="flex flex-col items-start gap-2">
        <p className="text-caption text-fg-muted">{t(draft?.period ? "pipelines.workbench.schedule.byPeriod" : "pipelines.workbench.schedule.hints.none", { period: draft?.period ?? "" })}</p>
        <Button
          size="sm"
          variant="secondary"
          onClick={() => {
            setOpen(true);
            onChange(scheduled(draft, FIRST_SCHEDULE));
          }}
        >
          {t("pipelines.workbench.schedule.add")}
        </Button>
      </div>
    );
  }
  const widget = {
    id: "workbench-schedule",
    name: "schedule",
    value: draft?.schedule,
    onChange: (value: unknown) => onChange(scheduled(draft, typeof value === "string" && value !== "" ? value : undefined)),
    onBlur: () => undefined,
    onFocus: () => undefined,
    rawErrors: below ? [t("pipelines.workbench.schedule.belowFloor", { gap, floor })] : undefined,
  } as unknown as WidgetProps;
  return (
    <FormProjectContext.Provider value={project}>
      <FormDataContext.Provider value={draft}>
        {below ? (
          <Alert tone="danger" role="alert">
            {t("pipelines.workbench.schedule.belowFloor", { gap, floor })}
          </Alert>
        ) : null}
        <Field
          id="workbench-schedule"
          label={t("pipelines.workbench.schedule.field")}
          description={t("pipelines.workbench.schedule.hints.field")}
        >
          <CronScheduleWidget {...widget} />
        </Field>
        {typeof floor === "number" && !below ? (
          <p className="text-caption text-fg-subtle">{t("pipelines.workbench.schedule.floor", { floor })}</p>
        ) : null}
        {below ? null : (
          <Alert tone="info" role="status">
            {t("pipelines.workbench.schedule.afterApply")}
          </Alert>
        )}
        <Button size="sm" variant="ghost" className="self-start" onClick={() => {
            setOpen(false);
            onChange({ ...draft, schedule: undefined, class: draft?.class === "scheduled" ? "auto" : draft?.class });
          }}>
          {t("pipelines.workbench.schedule.remove")}
        </Button>
      </FormDataContext.Provider>
    </FormProjectContext.Provider>
  );
}
