import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { Alert, Button, Field, Input, Select } from "../ui";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../ui/Table";
import { Skeleton } from "../ui/Skeleton";
import { download } from "../../pages/models/diagramExport";
import { usageRefusal } from "../../api/spaceUsage";
import {
  SURFACES,
  WINDOWS,
  endpointUsageQuery,
  exportUsage,
  projectUsageQuery,
  thisMonth,
} from "../../api/endpointUsage";
import type { UsageWindow } from "../../api/endpointUsage";

function useNumber(): (n: number) => string {
  const { i18n } = useTranslation();
  const format = new Intl.NumberFormat(i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 0 });
  return (n) => format.format(n);
}

/** The window picker both views share: 7, 30 or 90 days ending today. */
function WindowPicker({
  id,
  value,
  onChange,
  className = "w-48",
}: {
  id: string;
  value: UsageWindow;
  onChange: (days: UsageWindow) => void;
  className?: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Field id={id} label={t("endpointUsage.window")} help={t("endpointUsage.hint.window")} className={className}>
      <Select
        id={id}
        value={value}
        onChange={(event) => onChange(Number(event.target.value) as UsageWindow)}
      >
        {WINDOWS.map((days) => (
          <option key={days} value={days}>
            {t("endpointUsage.days", { count: days })}
          </option>
        ))}
      </Select>
    </Field>
  );
}

/** One bar per day, tallest at the window's busiest day; the numbers are in the table below. */
function Bars({ daily, label, small }: { daily: number[]; label: string; small?: boolean }): JSX.Element {
  const top = Math.max(1, ...daily);
  return (
    <svg
      role="img"
      aria-label={label}
      viewBox={`0 0 ${daily.length} 100`}
      preserveAspectRatio="none"
      className={`${small ? "h-8" : "h-24"} w-full rounded-md bg-surface-muted`}
    >
      {daily.map((n, i) => {
        const height = n > 0 ? Math.max(4, (n / top) * 100) : 0;
        // A day's bar has no identity beyond its place in the window.
        return <rect key={i} x={i + 0.15} width={0.7} y={100 - height} height={height} className="fill-primary-soft-fg" />;
      })}
    </svg>
  );
}

/** What nobody called yet says, with where the counts come from. */
function NoCalls(): JSX.Element {
  const { t } = useTranslation();
  return <p className="text-sm text-fg-muted">{t("endpointUsage.noCalls")}</p>;
}

/**
 * One Endpoint's answered calls per day and surface (T-5118): a bar per day, the totals per
 * surface and the days as a table for a screen reader and for exact numbers.
 */
export function EndpointUsageSection({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const number = useNumber();
  const [days, setDays] = useState<UsageWindow>(30);
  const usage = useQuery(endpointUsageQuery(project, name, days));
  const data = usage.data;
  return (
    <div className="space-y-3">
      <WindowPicker id="endpoint-usage-window" value={days} onChange={setDays} />
      {usage.isPending ? <Skeleton className="h-24 w-full" /> : null}
      {usage.isError ? (
        <Alert tone="danger">{t("endpointUsage.failed", { reason: usageRefusal(usage.error) })}</Alert>
      ) : null}
      {data ? (
        data.total === 0 ? (
          <NoCalls />
        ) : (
          <>
            <p className="text-sm">
              {t("endpointUsage.total", { count: data.total, calls: number(data.total), from: data.from, to: data.to })}
            </p>
            <Bars
              daily={data.days.map((day) => day.total)}
              label={t("endpointUsage.chart", { calls: number(data.total), days })}
            />
            <dl className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {SURFACES.filter((surface) => (data.totals[surface] ?? 0) > 0).map((surface) => (
                <div key={surface} className="min-w-0">
                  <dt className="text-caption text-fg-muted">{t(`endpointUsage.surface.${surface}`)}</dt>
                  <dd className="text-lg font-semibold tabular-nums">{number(data.totals[surface] ?? 0)}</dd>
                </div>
              ))}
            </dl>
            <details>
              <summary className="cursor-pointer text-sm">{t("endpointUsage.perDay")}</summary>
              <Table caption={t("endpointUsage.perDay")}>
                <TableHead>
                    <TableHeaderCell>{t("endpointUsage.day")}</TableHeaderCell>
                    <TableHeaderCell>{t("endpointUsage.calls")}</TableHeaderCell>
                  </TableHead>
                <TableBody>
                  {data.days
                    .filter((day) => day.total > 0)
                    .map((day) => (
                      <TableRow key={day.day}>
                        <TableCell>{day.day}</TableCell>
                        <TableCell className="tabular-nums">{number(day.total)}</TableCell>
                      </TableRow>
                    ))}
                </TableBody>
              </Table>
            </details>
          </>
        )
      ) : null}
      <p className="text-caption text-fg-muted">{t("endpointUsage.privacy")}</p>
    </div>
  );
}

const SUBGRID_FIELD = "sm:row-span-4 sm:grid sm:grid-rows-subgrid";

/**
 * Every Endpoint of the project the person may read, with its calls over the window, and a month
 * of all of them as CSV (T-5118).
 */
export function ProjectUsageSection({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const number = useNumber();
  const [days, setDays] = useState<UsageWindow>(30);
  const [month, setMonth] = useState(thisMonth());
  const [exporting, setExporting] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const usage = useQuery(projectUsageQuery(project, days));
  const monthValid = /^\d{4}-(0[1-9]|1[0-2])$/.test(month);

  const save = async () => {
    setExporting(true);
    setFailed(null);
    try {
      download(await exportUsage(project, month), `${project}-usage-${month}.csv`, "text/csv;charset=utf-8");
    } catch (error) {
      setFailed(usageRefusal(error));
    } finally {
      setExporting(false);
    }
  };

  const endpoints = usage.data?.endpoints ?? [];
  return (
    <section className="space-y-3" aria-labelledby="project-usage-title">
      <h2 id="project-usage-title" className="text-title font-semibold text-fg">
        {t("endpointUsage.title")}
      </h2>
      <p className="text-sm text-fg-muted">{t("endpointUsage.lead")}</p>
      {/* From sm up each Field spans four subgrid rows (label, control, hint, error), so the labels
          share a top line and the controls one line whatever their hints say; the button sits on
          the control row. Below sm the three stack (T-6310). */}
      <div className="grid gap-x-3 gap-y-1.5 sm:grid-cols-[12rem_12rem_auto] sm:grid-rows-[repeat(4,auto)] sm:justify-start">
        <WindowPicker id="project-usage-window" value={days} onChange={setDays} className={SUBGRID_FIELD} />
        <Field
          id="project-usage-month"
          label={t("endpointUsage.month")}
          help={t("endpointUsage.hint.month")}
          errors={monthValid ? undefined : [t("endpointUsage.monthInvalid")]}
          className={SUBGRID_FIELD}
        >
          <Input id="project-usage-month" type="month" value={month} onChange={(e) => setMonth(e.target.value)} />
        </Field>
        <Button variant="secondary" className="justify-self-start sm:col-start-3 sm:row-start-2" onClick={() => void save()}
          disabled={exporting || !monthValid}
          title={exporting ? t("endpointUsage.exporting") : monthValid ? undefined : t("endpointUsage.monthInvalid")}
        >
          {exporting ? t("endpointUsage.exporting") : t("endpointUsage.export")}
        </Button>
      </div>
      {failed ? <Alert tone="danger">{t("endpointUsage.failed", { reason: failed })}</Alert> : null}
      {usage.isPending ? <Skeleton className="h-24 w-full" /> : null}
      {usage.isError ? (
        <Alert tone="danger">{t("endpointUsage.failed", { reason: usageRefusal(usage.error) })}</Alert>
      ) : null}
      {usage.data && endpoints.length === 0 ? (
        <p className="text-sm text-fg-muted">{t("endpointUsage.noEndpoints")}</p>
      ) : null}
      {endpoints.length > 0 ? (
        <Table caption={t("endpointUsage.title")}>
          <TableHead>
              <TableHeaderCell>{t("endpointUsage.endpoint")}</TableHeaderCell>
              <TableHeaderCell>{t("endpointUsage.calls")}</TableHeaderCell>
              <TableHeaderCell>{t("endpointUsage.trend")}</TableHeaderCell>
            </TableHead>
          <TableBody>
            {endpoints.map((endpoint) => (
              <TableRow key={endpoint.name}>
                <TableCell primary>
                  <Link to="/projects/$project/$plural/$name" params={{ plural: "endpoints", project, name: endpoint.name }}>
                    {endpoint.name}
                  </Link>
                </TableCell>
                <TableCell className="tabular-nums">{number(endpoint.total)}</TableCell>
                <TableCell className="w-40">
                  {endpoint.total > 0 ? (
                    <Bars
                      small
                      daily={endpoint.daily}
                      label={t("endpointUsage.chart", { calls: number(endpoint.total), days })}
                    />
                  ) : (
                    <span className="text-caption text-fg-muted">{t("endpointUsage.none")}</span>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
      <p className="text-caption text-fg-muted">{t("endpointUsage.privacy")}</p>
    </section>
  );
}
