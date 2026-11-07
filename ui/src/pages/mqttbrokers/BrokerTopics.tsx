import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, forPeople, unwrap } from "../../api/client";
import { usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import { RecordLink } from "../../components/RecordLink";
import {
  Alert,
  Badge,
  Button,
  CodeBlock,
  ConfirmDialog,
  EmptyState,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

type Binding = components["schemas"]["Binding"];
type Hop = components["schemas"]["Hop"];
type Captured = components["schemas"]["Captured"];
type CapturedMessage = components["schemas"]["CapturedMessage"];

/** Whether MQTT topic `topic` matches filter `filter`: `+` one level, a final `#` the rest. */
export function topicMatches(filter: string, topic: string): boolean {
  const want = filter.split("/");
  const have = topic.split("/");
  for (let i = 0; i < want.length; i += 1) {
    if (want[i] === "#" && i === want.length - 1) return true;
    if (i >= have.length || (want[i] !== "+" && want[i] !== have[i])) return false;
  }
  return want.length === have.length;
}

/** A payload as a person reads it: JSON indented, any other text as it came, base64 as it is. */
export function readablePayload(message: Pick<CapturedMessage, "payload" | "encoding">): string {
  if (message.encoding !== "utf-8") return message.payload;
  try {
    return JSON.stringify(JSON.parse(message.payload), null, 2);
  } catch {
    return message.payload;
  }
}

function useTime(): Intl.DateTimeFormat {
  const { i18n } = useTranslation();
  return new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language ?? "en", { dateStyle: "short", timeStyle: "medium" });
}

/**
 * What a broker's topics feed (T-5533, API/01 §47) and, for a person who may approve brokers,
 * what a capture broker kept. A person who may not approve sees no capture panel and the Portal
 * never asks for one.
 */
export function BrokerTopics({ project, broker, mode }: { project: string; broker: string; mode: string }): JSX.Element {
  const permissions = usePermissions(project);
  // Only a read permissions document opens the panel: `can` says yes while it is still unknown,
  // and the capture route answers 404 to anyone it should not reach (PL-87).
  const approver = Array.isArray(permissions.data?.grants) && permissions.can("MqttBroker", "approve");
  const capture = approver && mode === "capture";
  const captured = useQuery({
    queryKey: ["mqttBrokerCaptured", project, broker],
    enabled: capture,
    refetchInterval: 15_000,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/mqttbrokers/{name}/captured", {
          params: { path: { project, name: broker }, query: { limit: 100 } },
        }),
      ),
  });
  return (
    <>
      <TopicsTable project={project} broker={broker} messages={captured.data?.messages} />
      {approver ? <CapturedPanel project={project} broker={broker} mode={mode} captured={captured} /> : null}
    </>
  );
}

function TopicsTable({ project, broker, messages }: { project: string; broker: string; messages?: CapturedMessage[] }): JSX.Element {
  const { t } = useTranslation();
  const time = useTime();
  const bindings = useQuery({
    queryKey: ["mqttBrokerBindings", project, broker],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/mqttbrokers/{name}/bindings", {
          params: { path: { project, name: broker } },
        }),
      ),
  });
  const heading = <h3 className="border-t border-border pt-3 text-body font-semibold text-fg">{t("mqttbrokers.topics.title")}</h3>;
  if (bindings.isPending) {
    return (
      <div aria-busy="true" className="flex flex-col gap-2" data-topics="loading">
        {heading}
        <span className="sr-only">{t("app.loading")}</span>
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }
  if (bindings.isError) {
    return (
      <Alert role="alert" tone="danger" title={t("mqttbrokers.topics.errorTitle")} data-topics="error">
        {bindings.error instanceof Error ? forPeople(bindings.error.message) : t("mqttbrokers.topics.error")}
      </Alert>
    );
  }
  const rows = bindings.data.bindings ?? [];
  const last = (filter?: string) => {
    const newest = filter ? messages?.find((message) => topicMatches(filter, message.topic)) : undefined;
    return newest ? time.format(new Date(newest.receivedAt)) : "—";
  };
  return (
    <div className="flex flex-col gap-2" data-topics={rows.length > 0 ? "bound" : "empty"}>
      {heading}
      {rows.length === 0 ? (
        <EmptyState bare title={t("mqttbrokers.topics.empty")} description={t("mqttbrokers.topics.emptyHint", { broker })} />
      ) : (
        <Table stack caption={t("mqttbrokers.topics.caption", { broker })}>
          <TableHead>
            <TableHeaderCell>{t("mqttbrokers.topics.filter")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.topics.via")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.topics.type")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.topics.attributes")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.topics.endpoint")}</TableHeaderCell>
            {messages ? <TableHeaderCell>{t("mqttbrokers.topics.last")}</TableHeaderCell> : null}
          </TableHead>
          <TableBody>
            {rows.map((row, index) => (
              <TableRow key={`${row.topicFilter ?? "hidden"}-${row.pipeline?.name ?? ""}-${row.endpoint?.name ?? ""}-${index}`}>
                <TableCell primary>
                  {row.topicFilter ? <code className="font-mono text-caption">{row.topicFilter}</code> : <HopCell project={project} hop={row.dataSource} />}
                </TableCell>
                <TableCell data-label={t("mqttbrokers.topics.via")}>
                  <Via project={project} row={row} />
                </TableCell>
                <TableCell data-label={t("mqttbrokers.topics.type")}>{row.type ?? "—"}</TableCell>
                <TableCell data-label={t("mqttbrokers.topics.attributes")}>
                  <Attributes row={row} />
                </TableCell>
                <TableCell data-label={t("mqttbrokers.topics.endpoint")}>{row.endpoint ? <HopCell project={project} hop={row.endpoint} /> : "—"}</TableCell>
                {messages ? <TableCell data-label={t("mqttbrokers.topics.last")}>{last(row.topicFilter ?? undefined)}</TableCell> : null}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  );
}

const PLURAL: Record<string, string> = { DataSource: "datasources", Pipeline: "pipelines", Endpoint: "endpoints" };

function HopCell({ project, hop }: { project: string; hop: Hop }): JSX.Element {
  const { t } = useTranslation();
  if (hop.hidden || !hop.name) {
    return <Badge tone="neutral">{t("mqttbrokers.topics.hidden", { kind: hop.kind })}</Badge>;
  }
  return <RecordLink project={project} plural={PLURAL[hop.kind] ?? "datasources"} name={hop.name} />;
}

/** The data source and the pipeline a topic goes through; a source no pipeline reads says so. */
function Via({ project, row }: { project: string; row: Binding }): JSX.Element {
  const { t } = useTranslation();
  if (!row.topicFilter) return <>—</>;
  return (
    <span className="flex flex-col gap-0.5">
      <HopCell project={project} hop={row.dataSource} />
      {row.pipeline ? (
        <HopCell project={project} hop={row.pipeline} />
      ) : (
        <span className="text-caption text-fg-muted">{t("mqttbrokers.topics.noPipeline")}</span>
      )}
    </span>
  );
}

export function Attributes({ row }: { row: Binding }): JSX.Element {
  const { t } = useTranslation();
  if (!row.attributes) {
    return row.pipeline?.name && row.endpoint?.name ? (
      <span className="text-caption text-fg-muted">{t("mqttbrokers.topics.byHand")}</span>
    ) : (
      <>—</>
    );
  }
  return (
    <ul className="flex flex-wrap gap-1" aria-label={t("mqttbrokers.topics.attributes")}>
      {row.attributes.map((attribute) => (
        <li key={attribute.to}>
          <Badge tone="info">
            {attribute.from ? t("mqttbrokers.topics.mapped", { from: attribute.from, to: attribute.to }) : t("mqttbrokers.topics.computed", { to: attribute.to })}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

interface CapturedQuery {
  data?: Captured;
  isPending: boolean;
  isError: boolean;
  error: unknown;
}

function CapturedPanel({ project, broker, mode, captured }: { project: string; broker: string; mode: string; captured: CapturedQuery }): JSX.Element {
  const { t } = useTranslation();
  const heading = <h3 className="border-t border-border pt-3 text-body font-semibold text-fg">{t("mqttbrokers.captured.title")}</h3>;
  if (mode !== "capture") {
    return (
      <div className="flex flex-col gap-2" data-captured="off">
        {heading}
        <p className="text-body text-fg-muted">{t("mqttbrokers.captured.off", { broker })}</p>
      </div>
    );
  }
  if (captured.isPending) {
    return (
      <div aria-busy="true" className="flex flex-col gap-2" data-captured="loading">
        {heading}
        <span className="sr-only">{t("app.loading")}</span>
        <Skeleton className="h-16 w-full" />
      </div>
    );
  }
  if (captured.isError || !captured.data) {
    return (
      <Alert role="alert" tone="danger" title={t("mqttbrokers.captured.errorTitle")} data-captured="error">
        {captured.error instanceof Error ? forPeople(captured.error.message) : t("mqttbrokers.captured.error")}
      </Alert>
    );
  }
  return <CapturedList project={project} broker={broker} captured={captured.data} heading={heading} />;
}

export function CapturedList({ project, broker, captured, heading }: { project: string; broker: string; captured: Captured; heading: JSX.Element }): JSX.Element {
  const { t } = useTranslation();
  const time = useTime();
  const [asking, setAsking] = useState(false);
  const client = useQueryClient();
  const clear = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/mqttbrokers/{name}/captured", {
          params: { path: { project, name: broker } },
        }),
      ),
    onSuccess: async () => {
      setAsking(false);
      await client.invalidateQueries({ queryKey: ["mqttBrokerCaptured", project, broker] });
    },
  });
  const messages = captured.messages;
  return (
    <div className="flex flex-col gap-2" data-captured={messages.length > 0 ? "messages" : "empty"}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        {heading}
        {messages.length > 0 ? (
          <Button size="sm" variant="secondary" onClick={() => setAsking(true)}>
            {t("mqttbrokers.captured.clear")}
          </Button>
        ) : null}
      </div>
      <p className="text-caption text-fg-muted">{t("mqttbrokers.captured.hint")}</p>
      {clear.isError ? (
        <Alert role="alert" tone="danger" title={t("mqttbrokers.captured.clearFailed")}>
          {clear.error instanceof Error ? forPeople(clear.error.message) : t("mqttbrokers.captured.error")}
        </Alert>
      ) : null}
      {messages.length === 0 ? (
        <EmptyState bare title={t("mqttbrokers.captured.empty")} description={t("mqttbrokers.captured.emptyHint", { broker })} />
      ) : (
        <ol className="flex flex-col gap-3" aria-label={t("mqttbrokers.captured.title")}>
          {messages.map((message, index) => (
            <li key={`${message.topic}-${message.receivedAt}-${index}`} className="flex min-w-0 flex-col gap-1">
              <span className="flex flex-wrap items-center gap-2 text-caption">
                <code className="min-w-0 break-all font-mono">{message.topic}</code>
                <time dateTime={message.receivedAt} className="text-fg-muted">
                  {time.format(new Date(message.receivedAt))}
                </time>
                {message.clientId ? <span className="text-fg-muted">{message.clientId}</span> : null}
                <span className="text-fg-muted">{t("mqttbrokers.captured.bytes", { count: message.bytes })}</span>
                {message.encoding === "base64" ? <Badge tone="neutral">base64</Badge> : null}
              </span>
              <CodeBlock
                label={t("mqttbrokers.captured.payload", { topic: message.topic })}
                className="max-h-64 max-w-full overflow-auto rounded-md bg-surface-subtle p-2 font-mono text-caption"
              >
                {readablePayload(message)}
              </CodeBlock>
            </li>
          ))}
        </ol>
      )}
      {captured.truncated ? <p className="text-caption text-fg-muted">{t("mqttbrokers.captured.truncated")}</p> : null}
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={t("mqttbrokers.captured.confirmTitle", { broker })}
        description={t("mqttbrokers.captured.confirmText")}
        confirmLabel={t("mqttbrokers.captured.clear")}
        pending={clear.isPending}
        onConfirm={() => clear.mutate()}
      />
    </div>
  );
}
