import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../api/client";
import {
  ACTIVITY_KINDS,
  ACTIVITY_SOURCES,
  activitySearch,
  changeOf,
  mergeActivity,
  objectOf,
  subscribeActivity,
} from "../api/activity";
import type { ActivityEvent, ActivityQuery, Severity } from "../api/activity";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Field,
  fieldIds,
  Icon,
  Select,
  Switch,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "./ui";
import { RecordLink } from "./RecordLink";
import type { BadgeTone } from "./ui";

const COLUMNS = 4;

/**
 * What happened, in words (T-2756): "Catalogue publication", and "Catalogue publication failed"
 * when the event is an error its kind does not already name. A kind this Portal does not know yet
 * shows as the platform wrote it.
 */
export function kindLabel(
  event: Pick<ActivityEvent, "kind" | "severity">,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const what = t(`activity.kinds.${event.kind}`, { defaultValue: event.kind });
  const namesFailure = /\.(error|denied)$/.test(event.kind);
  return event.severity === "error" && !namesFailure
    ? t("activity.failed", { what })
    : what;
}

/**
 * The event's sentence and, apart, the cause it was glued to (T-4200). A publisher writes "Endpoint
 * x was not published: <reason>" and keeps the reason in `details.reason`, which may be a CKAN API
 * name, a URL or a problem body: the row says the sentence, the cause waits behind a disclosure.
 * A rejection's reason is what the approver wrote to a person (T-5613): it stays in the sentence.
 */
export function sentenceOf(event: Pick<ActivityEvent, "summary" | "details" | "kind">): {
  sentence: string;
  reason?: string;
} {
  const reason = (event.details as { reason?: unknown } | undefined)?.reason;
  if (typeof reason !== "string" || !reason || event.kind === "change.rejected")
    return { sentence: event.summary };
  const at = event.summary.lastIndexOf(`: ${reason}`);
  if (at <= 0 || at + reason.length + 2 !== event.summary.length)
    return { sentence: event.summary };
  return { sentence: `${event.summary.slice(0, at)}.`, reason };
}

/** One row of the feed: an event, and how often the same thing happened again (T-2760). */
export interface ActivityRow {
  event: ActivityEvent;
  /** How many identical events the row stands for, itself included. */
  count: number;
  /** When the oldest of them happened. */
  since: string;
}

/**
 * Identical events as one row (T-2760): a CKAN publisher failing every minute filled the feed
 * with sixty rows an hour of the same sentence, and pushed everything else off it. Identical is
 * the same kind, source, severity, sentence and object; the row keeps the newest time and says
 * how many there were since the oldest. The events arrive newest first.
 */
export function groupRepeats(events: ActivityEvent[]): ActivityRow[] {
  const rows: ActivityRow[] = [];
  const byKey = new Map<string, ActivityRow>();
  for (const event of events) {
    const key = JSON.stringify([
      event.kind,
      event.source,
      event.severity,
      event.summary,
      objectOf(event) ?? "",
    ]);
    const seen = byKey.get(key);
    if (seen) {
      seen.count += 1;
      seen.since = event.time;
    } else {
      const row = { event, count: 1, since: event.time };
      byKey.set(key, row);
      rows.push(row);
    }
  }
  return rows;
}

const SEVERITY_TONE: Record<string, BadgeTone> = {
  info: "neutral",
  warning: "warning",
  error: "danger",
};

/** One page of activity, its filters and its live tail (UI-31). */
export function ActivityFeed({
  project,
  fixed,
  compact,
  limit = 50,
  initialKind,
}: {
  project: string;
  /** What this feed never leaves: an object page shows only its own object's events. */
  fixed?: ActivityQuery;
  /** An object page's panel: no filters, no tail toggle, just the last events. */
  compact?: boolean;
  limit?: number;
  /** The kind filter a link opened the page with (`?kind=mcp.tool`); an unknown kind is ignored. */
  initialKind?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const ids = useId();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const [kind, setKind] = useState(
    (ACTIVITY_KINDS as readonly string[]).includes(initialKind ?? "") ? (initialKind ?? "") : "",
  );
  const [source, setSource] = useState("");
  const [severity, setSeverity] = useState("");
  const [connector, setConnector] = useState("");
  const [tailing, setTailing] = useState(!compact);
  const [live, setLive] = useState<ActivityEvent[]>([]);
  const [arrived, setArrived] = useState(0);

  const query = useMemo<ActivityQuery>(
    () => ({
      ...fixed,
      kind: kind || fixed?.kind,
      source: source || fixed?.source,
      severity: (severity || fixed?.severity) as Severity | undefined,
      connector: connector || fixed?.connector,
      limit,
    }),
    [fixed, kind, source, severity, connector, limit],
  );

  const page = useQuery({
    queryKey: ["projects", project, "activity", activitySearch(query)],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/activity", {
          params: { path: { project }, query: activitySearch(query) },
        }),
      ),
  });

  // The connector filter offers the reader's own connectors; the server lists only those.
  const connectors = useQuery({
    queryKey: ["connectors"],
    enabled: !compact,
    queryFn: async () => unwrap(await api.GET("/api/v1/connectors", {})),
  });
  const ownConnectors = connectors.data?.items ?? [];

  // A filter change starts the tail again: what arrived under the old filter is not this list.
  const filterKey = JSON.stringify(activitySearch(query));
  const lastFilter = useRef(filterKey);
  useEffect(() => {
    if (lastFilter.current !== filterKey) {
      lastFilter.current = filterKey;
      setLive([]);
      setArrived(0);
    }
  }, [filterKey]);

  useEffect(() => {
    if (!tailing) {
      return;
    }
    return subscribeActivity(project, query, (event) => {
      setLive((known) => {
        const next = mergeActivity(known, [event]);
        return next === known ? known : next;
      });
      setArrived((count) => count + 1);
    });
  }, [project, tailing, filterKey, query]);

  const items = useMemo(
    () =>
      mergeActivity((page.data?.items ?? []) as ActivityEvent[], live).slice(
        0,
        limit,
      ),
    [page.data, live, limit],
  );

  const head = (
    <TableHead>
      {/* On a phone the time, kind and source are one line above the sentence, so the sentence
          gets the row's width (T-4165, photo audit T-4157). */}
      <TableHeaderCell secondary>{t("activity.time")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("activity.kind")}</TableHeaderCell>
      <TableHeaderCell>{t("activity.summary")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("activity.source")}</TableHeaderCell>
    </TableHead>
  );

  if (page.isPending) {
    return (
      <Table caption={t("activity.title")} status={t("app.loading")}>
        {head}
        <TableSkeleton columns={COLUMNS} />
      </Table>
    );
  }

  if (page.isError) {
    const message =
      page.error instanceof ApiError
        ? (page.error.problem?.detail ?? page.error.message)
        : t("app.error.generic");
    return (
      <Alert
        role="alert"
        tone="danger"
        actions={
          <Button
            size="sm"
            icon={<Icon name="refresh" className="size-4" />}
            onClick={() => {
              void page.refetch();
            }}
          >
            {t("app.error.retry")}
          </Button>
        }
      >
        {message}
      </Alert>
    );
  }

  const time = new Intl.DateTimeFormat(locale, {
    dateStyle: "short",
    timeStyle: "medium",
  });

  const filters = compact ? null : (
    <div className="flex flex-wrap items-end gap-3">
      <Field
        id={`${ids}-kind`}
        label={t("activity.filter.kind")}
        className="w-56"
      >
        <Select
          id={`${ids}-kind`}
          value={kind}
          onChange={(event) => setKind(event.target.value)}
        >
          <option value="">{t("activity.filter.all")}</option>
          {ACTIVITY_KINDS.map((option) => (
            <option key={option} value={option}>
              {t(`activity.kinds.${option}`)}
            </option>
          ))}
        </Select>
      </Field>
      <Field
        id={`${ids}-source`}
        label={t("activity.filter.source")}
        className="w-44"
      >
        <Select
          id={`${ids}-source`}
          value={source}
          onChange={(event) => setSource(event.target.value)}
        >
          <option value="">{t("activity.filter.all")}</option>
          {ACTIVITY_SOURCES.map((option) => (
            <option key={option} value={option}>
              {t(`activity.sources.${option}`)}
            </option>
          ))}
        </Select>
      </Field>
      <Field
        id={`${ids}-severity`}
        label={t("activity.filter.severity")}
        className="w-44"
      >
        <Select
          id={`${ids}-severity`}
          value={severity}
          onChange={(event) => setSeverity(event.target.value)}
        >
          <option value="">{t("activity.filter.all")}</option>
          {(["info", "warning", "error"] as const).map((option) => (
            <option key={option} value={option}>
              {t(`activity.severity.${option}`)}
            </option>
          ))}
        </Select>
      </Field>
      {/* A filter with nothing to choose is no filter: shown once the reader has a connector. */}
      {ownConnectors.length > 0 ? (
        <Field
          id={`${ids}-connector`}
          label={t("activity.filter.connector")}
          description={t("activity.filter.connectorHint")}
          className="w-56"
        >
          <Select
            id={`${ids}-connector`}
            value={connector}
            aria-describedby={fieldIds(`${ids}-connector`).description}
            onChange={(event) => setConnector(event.target.value)}
          >
            <option value="">{t("activity.filter.all")}</option>
            {ownConnectors.map((option) => (
              <option key={option.id} value={option.id}>
                {option.name}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      <Switch
        checked={tailing}
        onCheckedChange={setTailing}
        label={t("activity.tail")}
        aria-label={t("activity.tail")}
        className="mb-2"
      />
      <span className="mb-1.5 text-caption text-fg-subtle">
        {tailing ? t("activity.tailOn") : t("activity.tailOff")}
      </span>
    </div>
  );

  // Only an error interrupts a reader: the tail announces nothing else (a11y, UI-31).
  const criticals = live.filter((event) => event.severity === "error").length;

  return (
    <div className="flex flex-col gap-4">
      {filters}
      <p
        role="status"
        aria-live="polite"
        data-testid="activity-announce"
        className="sr-only"
      >
        {criticals > 0 ? t("activity.announce", { count: criticals }) : ""}
      </p>
      <span data-testid="activity-arrived" hidden>
        {arrived}
      </span>
      {items.length === 0 ? (
        <EmptyState
          icon="refresh"
          title={compact ? t("activity.panelEmpty") : t("activity.empty")}
          description={
            compact ? t("activity.panelEmptyHint") : t("activity.emptyHint")
          }
        />
      ) : (
        <Table caption={t("activity.title")}>
          {head}
          <TableBody>
            {groupRepeats(items).map(({ event, count, since }) => {
              const object = objectOf(event);
              const { sentence, reason } = sentenceOf(event);
              const change = changeOf(event);
              return (
                <TableRow key={`${event.time}-${event.kind}-${event.summary}`}>
                  <TableCell secondary className="whitespace-nowrap text-fg-muted">
                    {time.format(new Date(event.time))}
                  </TableCell>
                  <TableCell secondary>
                    <Badge
                      tone={SEVERITY_TONE[event.severity] ?? "neutral"}
                      data-testid="activity-kind"
                    >
                      {kindLabel(event, t)}
                    </Badge>
                  </TableCell>
                  <TableCell primary>
                    <div className="mb-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-caption text-fg-muted sm:hidden">
                      <span>{time.format(new Date(event.time))}</span>
                      <Badge tone={SEVERITY_TONE[event.severity] ?? "neutral"}>
                        {kindLabel(event, t)}
                      </Badge>
                      <span>
                        {t(`activity.sources.${event.source}`, {
                          defaultValue: event.source,
                        })}
                      </span>
                    </div>
                    <span data-testid="activity-sentence">{sentence}</span>
                    {reason ? (
                      <details className="mt-0.5 text-caption text-fg-muted">
                        <summary className="cursor-pointer">
                          {t("activity.technical")}
                        </summary>
                        <p
                          className="mt-0.5 break-all font-mono"
                          data-testid="activity-reason"
                        >
                          {reason}
                        </p>
                      </details>
                    ) : null}
                    {count > 1 ? (
                      <div
                        className="mt-0.5 text-caption text-fg-muted"
                        data-testid="activity-repeats"
                      >
                        {t("activity.repeated", {
                          count,
                          since: time.format(new Date(since)),
                        })}
                      </div>
                    ) : null}
                    {object || change ? (
                      <div className="mt-0.5 flex flex-wrap gap-x-3">
                        {object ? (
                          // In the app, not a reload: the live tail and its cache stay (T-6230).
                          <RecordLink
                            project={project}
                            plural={object.split("/")[0]}
                            name={object.split("/")[1]}
                            className="font-mono text-caption"
                          >
                            {object}
                          </RecordLink>
                        ) : null}
                        {/* What went live, and who approved it, is on the change's own page (T-3292). */}
                        {change ? (
                          <Link
                            to="/projects/$project/approvals/$id"
                            params={{ project, id: change }}
                            className="focus-ring rounded-sm font-mono text-caption text-primary-soft-fg hover:underline"
                          >
                            {change}
                          </Link>
                        ) : null}
                      </div>
                    ) : null}
                  </TableCell>
                  <TableCell secondary className="text-fg-muted">
                    {t(`activity.sources.${event.source}`, {
                      defaultValue: event.source,
                    })}
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      )}
    </div>
  );
}
