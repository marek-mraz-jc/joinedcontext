import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { ApiError, api, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import { usePermissions, useReadableList } from "../../api/permissions";
import type { Manifest } from "../../api/manifest";
import type { components } from "../../api/schema";
import { CopyUrlButton } from "../../components/endpoints/links";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import {
  Alert,
  Badge,
  Button,
  CodeBlock,
  EmptyState,
  PageHeader,
  PageLoading,
  SourceLink,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { AddDevice } from "./AddDevice";
import { BrokerAccess } from "./BrokerAccess";
import { BrokerTopics } from "./BrokerTopics";
import {
  brokerAddress,
  Connections,
  publishExample,
  publishTopic,
} from "./MqttBrokersPage";
import { TestPublishDialog } from "./TestPublish";
import { holdersOf, rolesHeldBy } from "./brokerDraft";
import { BrokerEditor, LIMIT_DEFAULTS, Section, SECTIONS } from "./BrokerEditor";
import type { SectionId } from "./BrokerEditor";

type Measured = components["schemas"]["Measured"];
type Watched = components["schemas"]["Watched"];

interface Role {
  name?: string;
  serviceAccounts?: string[];
  publish?: string[];
  subscribe?: string[];
}

interface BrokerSpec {
  roles?: Role[];
  port?: number;
  hostname?: string;
  allowedSources?: string[];
  mode?: string;
  capture?: { messagesPerTopic?: number; window?: string };
  limits?: {
    maxMessageBytes?: number;
    messagesPerSecond?: number;
    maxConnections?: number;
    connectionsPerAddress?: number;
  };
}

/** One MQTT broker's own page (T-6103, MF-54, ADR-N-048): live use, its settings in words, how a device connects. */
export function MqttBrokerPage({
  project,
  name,
}: {
  project: string;
  name: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const [testing, setTesting] = useState(false);
  const [dirty, setDirty] = useState<SectionId[]>([]);
  const broker = useQuery({
    queryKey: queryKeys.resource(project, "mqttbrokers", name),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "mqttbrokers", name } },
        }),
      ),
  });
  const measured = useMeasured(project, name);
  const refusals = useRefusalsToday(project, name);
  const permissions = usePermissions(project);
  if (broker.isPending) return <PageLoading label={t("app.loading")} />;
  // The 404 hides whether the broker exists (PF-59); the caller's own grants say which sentence
  // applies, and those grants are theirs to know.
  if (broker.error instanceof ApiError && broker.error.status === 404) {
    if (permissions.checking) return <PageLoading label={t("app.loading")} />;
    return (
      <BrokerRefused
        project={project}
        name={name}
        noRight={!permissions.can("MqttBroker", "read")}
      />
    );
  }
  if (broker.isError) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("mqttbrokers.page.refused.purpose")}
        error={broker.error}
        onRetry={() => void broker.refetch()}
        back={<BackLink project={project} />}
      />
    );
  }
  const manifest = broker.data as Manifest;
  const spec = (manifest.spec ?? {}) as BrokerSpec;
  const mode = spec.mode ?? "ingest";
  const address = brokerAddress(
    name,
    project,
    window.location.hostname,
    spec.port,
    spec.hostname,
  );
  const title = localized(manifest.metadata.title, locale, name);
  return (
    <div className="flex flex-col gap-section">
      <BrokerHeader
        project={project}
        manifest={manifest}
        title={title}
        address={address}
        measured={measured.data}
        refusals={refusals.data}
        onTest={() => setTesting(true)}
      />
      <div className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-[11rem_minmax(0,1fr)]">
        <nav
          aria-label={t("mqttbrokers.page.sections")}
          className="lg:sticky lg:top-4 lg:self-start"
        >
          <ul className="flex flex-wrap gap-1 lg:flex-col">
            {SECTIONS.map((section) => (
              <li key={section} className="shrink-0">
                <a
                  href={`#broker-${section}`}
                  className="focus-ring touch-size block rounded-md px-2 py-1.5 text-body text-fg-muted hover:bg-surface-subtle hover:text-fg"
                >
                  {t(`mqttbrokers.page.section.${section}`)}
                  {dirty.includes(section) ? (
                    <span className="ml-1 inline-block size-2 rounded-full bg-warning" aria-label={t("mqttbrokers.edit.unsaved")} role="img" />
                  ) : null}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <div className="flex min-w-0 flex-col gap-4">
          <LiveNow
            project={project}
            name={name}
            spec={spec}
            measured={measured.data}
            failed={measured.isError}
            refusals={refusals.data}
          />
          <BrokerEditor
            project={project}
            manifest={manifest}
            measured={measured.data}
            onDirty={setDirty}
            rolesNote={<BrokerAccess project={project} broker={name} />}
          />
          <Section id="path" lead={t("mqttbrokers.connections.lead")}>
            <Connections project={project} brokers={[manifest]} bare />
            <BrokerTopics project={project} broker={name} mode={mode} />
          </Section>
          <Section id="connect">
            <ConnectFacts
              project={project}
              name={name}
              spec={spec}
              address={address}
            />
            <AddDevice
              project={project}
              broker={name}
              example={(client, topic) =>
                address ? publishExample(address, client, topic) : undefined
              }
            />
          </Section>
        </div>
      </div>
      {testing ? (
        <TestPublishDialog
          project={project}
          broker={name}
          roles={spec.roles ?? []}
          open={testing}
          onOpenChange={setTesting}
        />
      ) : null}
    </div>
  );
}

function BrokerHeader({
  project,
  manifest,
  title,
  address,
  measured,
  refusals,
  onTest,
}: {
  project: string;
  manifest: Manifest;
  title: string;
  address?: string;
  measured?: Measured;
  refusals?: Watched;
  onTest: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const name = manifest.metadata.name;
  const spec = (manifest.spec ?? {}) as BrokerSpec;
  const mode = spec.mode ?? "ingest";
  // The CNAME target of a custom hostname: the broker's platform name, which the edge's DNS answers.
  const platformHost = brokerAddress(
    name,
    project,
    window.location.hostname,
  )?.split(":")[0];
  const waitingForDns = (manifest.status?.conditions ?? []).some(
    (c) => c.reason === "WaitingForDns",
  );
  return (
    <div className="flex flex-col gap-2">
      {/* The breadcrumb leads back; the id sits beside the title (on a phone the breadcrumb ends with it). */}
      <PageHeader
        title={title}
        titleAction={<code className="ms-2 hidden font-mono text-caption text-fg-muted sm:inline">{name}</code>}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Button onClick={onTest}>{t("mqttbrokers.test.open")}</Button>
          </div>
        }
      />
      <div className="flex flex-wrap items-center gap-2">
        <LifecycleBadge kind="phase" value={manifest.status?.phase} />
        <StateBadge measured={measured} />
        {manifest.status?.sourceUrl ? (
          <SourceLink
            href={manifest.status.sourceUrl}
            label={t("mqttbrokers.page.source")}
          />
        ) : null}
        <Badge tone="info" data-mode={mode}>
          {t(`mqttAllowance.mode.${mode}`, { defaultValue: mode })}
        </Badge>
        {address ? (
          <span className="flex min-w-0 items-center gap-1">
            <code className="min-w-0 break-all font-mono text-caption">
              {address}
            </code>
            <CopyUrlButton
              url={address}
              label={t("mqttbrokers.connect.copy")}
            />
          </span>
        ) : null}
      </div>
      <LiveStrip measured={measured} refusals={refusals} />
      {waitingForDns && spec.hostname ? (
        <Alert
          tone="info"
          title={t("mqttbrokers.connect.waitingForDnsTitle")}
          data-dns="waiting"
        >
          {platformHost
            ? t("mqttbrokers.connect.waitingForDns", {
                host: spec.hostname,
                target: platformHost,
              })
            : t("mqttbrokers.connect.waitingForDnsNoTarget", {
                host: spec.hostname,
              })}
        </Alert>
      ) : null}
    </div>
  );
}

const FACTS =
  "grid grid-cols-[minmax(0,1fr)] gap-x-4 gap-y-2 text-body sm:grid-cols-[max-content_minmax(0,1fr)]";

/** The broker cannot be shown: the caller may not read brokers here, or none has this name. */
function BrokerRefused({
  project,
  name,
  noRight,
}: {
  project: string;
  name: string;
  noRight: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const why = noRight ? "noRight" : "missing";
  return (
    <div className="flex flex-col gap-4" data-refused={why}>
      <PageHeader
        title={name}
        description={t("mqttbrokers.page.refused.purpose")}
      />
      <EmptyState
        icon="access"
        title={t(`mqttbrokers.page.refused.${why}`, { name })}
        description={t(`mqttbrokers.page.refused.${why}Hint`, { project, name })}
        action={<BackLink project={project} />}
      />
    </div>
  );
}

function BackLink({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Link
      to="/projects/$project/$plural"
      params={{ project, plural: "mqttbrokers" }}
      className="focus-ring w-fit text-body text-primary-soft-fg underline-offset-2 hover:underline"
    >
      {t("mqttbrokers.page.back")}
    </Link>
  );
}

/** What the broker measures now, every 15 s (T-4043); shared by the header and Live now. */
function useMeasured(project: string, name: string) {
  return useQuery({
    queryKey: ["mqttBrokerMeasured", project, name],
    refetchInterval: 15_000,
    queryFn: async () =>
      unwrap(
        await api.GET(
          "/api/v1/projects/{project}/mqttbrokers/{name}/measured",
          {
            params: { path: { project, name } },
          },
        ),
      ),
  });
}

/** The gateway's refused sign-ins since local midnight (T-5534's watch), every minute. */
function useRefusalsToday(project: string, name: string) {
  return useQuery({
    queryKey: ["mqttBrokerRefusalsToday", project, name],
    refetchInterval: 60_000,
    queryFn: async () => {
      const midnight = new Date();
      midnight.setHours(0, 0, 0, 0);
      return unwrap(
        await api.GET("/api/v1/projects/{project}/mqttbrokers/{name}/watch", {
          params: {
            path: { project, name },
            query: { since: midnight.toISOString() },
          },
        }),
      );
    },
  });
}

/** Receiving while messages arrive, Idle while none do, Not measured when the broker did not answer. */
export function brokerState(
  measured?: Measured,
): "receiving" | "idle" | "unmeasured" | undefined {
  if (!measured) return undefined;
  if (!measured.measured) return "unmeasured";
  return (measured.messagesPerSecond ?? 0) > 0 ? "receiving" : "idle";
}

function StateBadge({ measured }: { measured?: Measured }): JSX.Element | null {
  const { t } = useTranslation();
  const state = brokerState(measured);
  if (!state) return null;
  const tone = state === "receiving" ? "success" : "neutral";
  return (
    <Badge tone={tone} data-state={state}>
      {t(`mqttbrokers.page.state.${state}`)}
    </Badge>
  );
}

function connectedNow(measured?: Measured): number {
  return (measured?.accounts ?? []).reduce(
    (sum, account) => sum + account.connected,
    0,
  );
}

/** The most frequent reason among `refusals`, or undefined when there is none. */
export function commonReason(
  refusals: Watched["refusals"],
): string | undefined {
  const counts = new Map<string, number>();
  for (const refusal of refusals)
    counts.set(refusal.reason, (counts.get(refusal.reason) ?? 0) + 1);
  return [...counts].sort((a, b) => b[1] - a[1])[0]?.[0];
}

function LiveStrip({
  measured,
  refusals,
}: {
  measured?: Measured;
  refusals?: Watched;
}): JSX.Element | null {
  const { t, i18n } = useTranslation();
  if (!measured?.measured) return null;
  const number = new Intl.NumberFormat(
    i18n.resolvedLanguage ?? i18n.language ?? "en",
    { maximumFractionDigits: 1 },
  );
  return (
    <p className="text-body text-fg-muted" data-testid="live-strip">
      {t("mqttbrokers.page.strip", {
        rate: number.format(measured.messagesPerSecond ?? 0),
        connected: connectedNow(measured),
        refused: refusals?.refusalsRead ? refusals.refusals.length : "—",
      })}
    </p>
  );
}

function LiveNow({
  project,
  name,
  spec,
  measured,
  failed,
  refusals,
}: {
  project: string;
  name: string;
  spec: BrokerSpec;
  measured?: Measured;
  failed: boolean;
  refusals?: Watched;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const number = new Intl.NumberFormat(
    i18n.resolvedLanguage ?? i18n.language ?? "en",
    { maximumFractionDigits: 1 },
  );
  const limits = { ...LIMIT_DEFAULTS, ...spec.limits };
  const reason = refusals?.refusalsRead
    ? commonReason(refusals.refusals)
    : undefined;
  return (
    <Section id="live">
      {measured && !measured.measured ? (
        <Alert
          tone="info"
          title={t("mqttbrokers.measured.not")}
          data-measured="no"
        >
          {measured.reason}
        </Alert>
      ) : failed ? (
        <Alert
          tone="info"
          title={t("mqttbrokers.measured.not")}
          data-measured="no"
        >
          {t("mqttbrokers.page.notRead")}
        </Alert>
      ) : null}
      {measured?.measured ? (
        <>
          <dl
            className="grid grid-cols-2 gap-3 lg:grid-cols-4"
            data-measured="yes"
          >
            <Figure
              label={t("mqttbrokers.page.figure.rate")}
              value={number.format(measured.messagesPerSecond ?? 0)}
              note={t("mqttbrokers.page.figure.rateNote", {
                limit: number.format(limits.messagesPerSecond),
              })}
            />
            <Figure
              label={t("mqttbrokers.page.figure.connected")}
              value={`${connectedNow(measured)} / ${number.format(limits.maxConnections)}`}
              note={t("mqttbrokers.page.figure.connectedNote")}
            />
            <Figure
              label={t("mqttbrokers.page.figure.refusedSignIns")}
              value={
                refusals?.refusalsRead ? String(refusals.refusals.length) : "—"
              }
              note={
                reason
                  ? t("mqttbrokers.page.figure.mostly", { reason })
                  : t("mqttbrokers.page.figure.refusedSignInsNote")
              }
              bad={
                refusals?.refusalsRead ? refusals.refusals.length > 0 : false
              }
            />
            <Figure
              label={t("mqttbrokers.page.figure.refusedPublishes")}
              value={
                measured.refusedPublishes == null
                  ? "—"
                  : number.format(measured.refusedPublishes)
              }
              note={t("mqttbrokers.page.figure.refusedPublishesNote")}
              bad={(measured.refusedPublishes ?? 0) > 0}
            />
          </dl>
          <RateChart rates={measured.rates} />
          <Devices project={project} name={name} spec={spec} measured={measured} />
        </>
      ) : null}
    </Section>
  );
}

function Figure({
  label,
  value,
  note,
  bad,
}: {
  label: string;
  value: string;
  note: string;
  bad?: boolean;
}): JSX.Element {
  return (
    <div
      className="flex flex-col gap-0.5 rounded-md border border-border p-3"
      data-figure={label}
    >
      <dt className="text-caption text-fg-muted">{label}</dt>
      <dd
        className={`text-title font-semibold tabular-nums ${bad ? "text-danger" : "text-fg"}`}
      >
        {value}
      </dd>
      <dd className="text-caption text-fg-muted">{note}</dd>
    </div>
  );
}

/** Messages per second over the samples the broker keeps, as one line; empty until two samples exist. */
export function RateChart({
  rates,
}: {
  rates: Measured["rates"];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const number = new Intl.NumberFormat(
    i18n.resolvedLanguage ?? i18n.language ?? "en",
    { maximumFractionDigits: 1 },
  );
  const quiet = rates.length >= 2 && rates.every((point) => point.perSecond === 0);
  if (quiet) {
    const span = new Date(rates[rates.length - 1]?.at ?? 0).getTime() - new Date(rates[0]?.at ?? 0).getTime();
    return (
      <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-body text-fg-muted" data-chart="quiet">
        {t("mqttbrokers.page.chartQuiet", { minutes: Math.max(1, Math.round(span / 60_000)) })}
      </p>
    );
  }
  if (rates.length < 2) {
    return (
      <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-body text-fg-muted" data-chart="empty">
        {t("mqttbrokers.page.chartEmpty")}
      </p>
    );
  }
  const first = new Date(rates[0]?.at ?? 0).getTime();
  const last = new Date(rates[rates.length - 1]?.at ?? 0).getTime();
  const top = Math.max(1, ...rates.map((point) => point.perSecond));
  const span = Math.max(1, last - first);
  const points = rates
    .map((point) => {
      const x = ((new Date(point.at).getTime() - first) / span) * 100;
      const y = 40 - (point.perSecond / top) * 38;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
  const minutes = Math.max(1, Math.round(span / 60_000));
  const now = rates[rates.length - 1]?.perSecond ?? 0;
  const summary = t("mqttbrokers.page.chartSummary", {
    minutes,
    top: number.format(top),
    now: number.format(now),
  });
  return (
    <figure className="flex flex-col gap-1" data-chart="rates">
      <figcaption className="text-caption text-fg-muted">{summary}</figcaption>
      <svg
        viewBox="0 0 100 42"
        preserveAspectRatio="none"
        className="h-24 w-full text-primary-soft-fg"
        role="img"
        aria-label={summary}
      >
        <line
          x1="0"
          x2="100"
          y1="2"
          y2="2"
          stroke="currentColor"
          strokeOpacity="0.2"
          strokeDasharray="1 1"
          vectorEffect="non-scaling-stroke"
        />
        <line
          x1="0"
          x2="100"
          y1="40"
          y2="40"
          stroke="currentColor"
          strokeOpacity="0.2"
          vectorEffect="non-scaling-stroke"
        />
        <polyline
          points={points}
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
    </figure>
  );
}

/** Each account of the broker's roles: its roles and its connected clients now; no client id or address. */
function Devices({
  project,
  name,
  spec,
  measured,
}: {
  project: string;
  name: string;
  spec: BrokerSpec;
  measured: Measured;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const time = new Intl.DateTimeFormat(
    i18n.resolvedLanguage ?? i18n.language ?? "en",
    { dateStyle: "short", timeStyle: "short" },
  );
  const accounts = asManifests(
    useReadableList(project, "serviceaccounts", "ServiceAccount").data?.items ?? [],
  );
  if (measured.accounts.length === 0) {
    return (
      <p className="text-body text-fg-muted" data-devices="empty">
        {t("mqttbrokers.page.noDevices")}
      </p>
    );
  }
  const rolesOf = (account: string) =>
    rolesHeldBy(account, spec.roles ?? [], accounts, name, project).map((role) => role.name ?? "");
  return (
    <Table stack caption={t("mqttbrokers.page.devices")}>
      <TableHead>
        <TableHeaderCell>{t("mqttbrokers.page.account")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.acl.role")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.measured.connected")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.page.since")}</TableHeaderCell>
      </TableHead>
      <TableBody>
        {measured.accounts.map((account) => (
          <TableRow
            key={account.serviceAccount}
            data-account={account.serviceAccount}
          >
            <TableCell primary>{account.serviceAccount}</TableCell>
            <TableCell data-label={t("mqttbrokers.acl.role")}>
              {rolesOf(account.serviceAccount).join(", ") || "—"}
            </TableCell>
            <TableCell data-label={t("mqttbrokers.measured.connected")}>
              {account.connected > 0 ? (
                <Badge tone="success">
                  {t("mqttbrokers.page.connectedCount", {
                    count: account.connected,
                  })}
                </Badge>
              ) : (
                <Badge tone="neutral">{t("mqttbrokers.page.offlineNow")}</Badge>
              )}
            </TableCell>
            <TableCell data-label={t("mqttbrokers.page.since")}>
              {account.since ? time.format(new Date(account.since)) : "—"}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function ConnectFacts({
  project,
  name,
  spec,
  address,
}: {
  project: string;
  name: string;
  spec: BrokerSpec;
  address?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const roles = spec.roles ?? [];
  const accounts = asManifests(
    useReadableList(project, "serviceaccounts", "ServiceAccount").data?.items ?? [],
  );
  // Who holds a role: its own list and every account a RoleBinding grants it (T-5872).
  const devices = [
    ...new Set(
      roles.flatMap((role) =>
        holdersOf(role, accounts, name, project).map((holder) => holder.account),
      ),
    ),
  ];
  // The example's topic is one the first device's own roles may publish to.
  const exampleTopic = publishTopic(rolesHeldBy(devices[0] ?? "", roles, accounts, name, project));
  const platformHost = brokerAddress(
    name,
    project,
    window.location.hostname,
  )?.split(":")[0];
  return (
    <dl className={FACTS}>
      <dt className="text-fg-muted">{t("mqttbrokers.connect.address")}</dt>
      <dd className="flex flex-wrap items-center gap-2">
        {address ? (
          <>
            <code className="min-w-0 break-all font-mono">{address}</code>
            <Badge tone="neutral">TLS</Badge>
            <CopyUrlButton
              url={address}
              label={t("mqttbrokers.connect.copy")}
            />
          </>
        ) : (
          t("mqttbrokers.connect.noAddress")
        )}
      </dd>
      {spec.hostname && platformHost ? (
        <>
          <dt className="text-fg-muted">{t("mqttbrokers.page.cname")}</dt>
          <dd className="break-all font-mono">{platformHost}</dd>
        </>
      ) : null}
      <dt className="text-fg-muted">{t("mqttbrokers.connect.certificate")}</dt>
      <dd className="break-words">
        {address
          ? t("mqttbrokers.connect.certificateIssued", {
              host: address.split(":")[0],
            })
          : "—"}
      </dd>
      <dt className="text-fg-muted">{t("mqttbrokers.connect.client")}</dt>
      <dd className="flex flex-col items-start">
        {devices.length > 0
          ? devices.map((account) => (
              <code key={account} className="whitespace-nowrap font-mono">
                {`${project}-${account}`}
              </code>
            ))
          : "—"}
      </dd>
      <dt className="text-fg-muted">{t("mqttbrokers.connect.password")}</dt>
      <dd>{t("mqttbrokers.connect.token")}</dd>
      {address && devices.length > 0 ? (
        <>
          <dt className="text-fg-muted">{t("mqttbrokers.connect.example")}</dt>
          <dd className="flex min-w-0 flex-col items-start gap-2">
            <CodeBlock
              label={t("mqttbrokers.connect.example")}
              className="max-w-full overflow-x-auto rounded-md bg-surface-subtle p-2 font-mono text-caption"
            >
              {publishExample(
                address,
                `${project}-${devices[0]}`,
                exampleTopic,
              )}
            </CodeBlock>
            <CopyUrlButton
              url={publishExample(
                address,
                `${project}-${devices[0]}`,
                exampleTopic,
              )}
              label={t("mqttbrokers.connect.copyExample")}
            />
            <span className="text-caption text-fg-muted">
              {t("mqttbrokers.connect.exampleHint")}
            </span>
          </dd>
        </>
      ) : null}
      <dt className="text-fg-muted">{t("mqttbrokers.connect.pipeline")}</dt>
      <dd>{t("mqttbrokers.connect.brokerRef", { broker: name })}</dd>
    </dl>
  );
}
