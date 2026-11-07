import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { forPeople } from "../../api/client";
import { asManifests } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useReadableList } from "../../api/permissions";
import { RecordLink, RECORD_LINK_STYLE } from "../../components/RecordLink";
import {
  Alert,
  Badge,
  Card,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { KindList } from "../../routes/ResourceListPage";
import { useMqttAllowance } from "./allowance";
import { connectionsOf } from "./connections";
import type { Edge } from "./connections";

/**
 * The project's MQTT brokers (T-3839, MF-54, ADR-N-048): the list with its create and edit forms,
 * each broker's topic access by role and how a device connects, and who talks to whom from a
 * device to the Policy that guards the write. A broker holds no secret, so nothing here shows one.
 */
export function MqttBrokersPage({ project }: { project: string }): JSX.Element {
  const brokers = useReadableList(project, "mqttbrokers", "MqttBroker");
  const items = asManifests(brokers.data?.items ?? []);
  const refused = useCreateRefused(project);
  return (
    <div className="flex flex-col gap-section">
      {/* The page's title comes first; what the project may still add sits under its brokers (owner 2026-10-30). */}
      <KindList project={project} plural="mqttbrokers" createRefused={refused} />
      <AllowancePanel project={project} />
      {items.length > 0 ? <Connections project={project} brokers={items} /> : null}
    </div>
  );
}

/**
 * Why New MQTT broker would be refused, in the allowance panel's own words (T-5589): MQTT off for
 * the project, or every allowed broker in use. Unknown while the allowance loads or failed: the
 * API's 403 stays the gate either way.
 */
function useCreateRefused(project: string): string | undefined {
  const { t } = useTranslation();
  const data = useMqttAllowance(project).data;
  if (data === undefined) return undefined;
  if (!data.enabled) return `${t("mqttAllowance.project.offTitle")}. ${t("mqttAllowance.project.off", { project })}`;
  if (data.brokers.left === 0) return t("mqttAllowance.project.full", { limit: data.brokers.limit });
  return undefined;
}

interface Role {
  name?: string;
  serviceAccounts?: string[];
  publish?: string[];
  subscribe?: string[];
}

/**
 * Where a device reaches the broker from outside: TLS on 8883 at `<name>.<project>.mqtt.<domain>`,
 * or a dedicated port (ADR-N-048 §5). The domain is the one the Portal is served under
 * (`portal.<domain>`); on any other host it is not known and no address is shown.
 */
export function brokerAddress(
  name: string,
  project: string,
  hostname: string,
  port?: number,
  customHostname?: string,
): string | undefined {
  // A city's own name answers on 8883 wherever the Portal runs (ADR-N-048 §2.11, T-5874).
  if (customHostname) return `${customHostname}:8883`;
  if (!hostname.startsWith("portal.")) return undefined;
  const domain = hostname.slice("portal.".length);
  return `${name}.${project}.mqtt.${domain}:${port ?? 8883}`;
}

/** The first topic a device may publish to, with `${clientid}` left for the shell to keep as typed. */
export function publishTopic(roles: Role[]): string {
  const filter = roles.flatMap((role) => role.publish ?? [])[0] ?? "devices/${clientid}/data";
  return filter.replace(/\/#$/, "/data").replace(/\+/g, "1");
}

/**
 * A mosquitto_pub line a device owner copies (T-4860): TLS with the system CA store, the client
 * id as user name and `$TOKEN` where the device's access token goes. No token is ever filled in.
 */
export function publishExample(address: string, client: string, topic: string): string {
  const [host, port] = address.split(":");
  return [
    `mosquitto_pub -h ${host} -p ${port}`,
    `  --capath /etc/ssl/certs -i ${client} -u ${client} -P "$TOKEN"`,
    `  -t ${shellQuoted(topic.replaceAll("${clientid}", client))} -m '{"temperature": 21.5}'`,
  ].join(" \\\n");
}

/** One shell word: single quotes, each ' inside closed, escaped and reopened. */
export function shellQuoted(text: string): string {
  return `'${text.replaceAll("'", "'\\''")}'`;
}

/**
 * What the project may still add (T-4860, API/01 §47): off with who can turn it on, or the
 * brokers, connections and message rate left, the modes and the addresses it may use.
 */
function AllowancePanel({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const allowance = useMqttAllowance(project);
  if (allowance.isPending) {
    return (
      <Card aria-busy="true" data-allowance="loading">
        <span className="sr-only">{t("app.loading")}</span>
        <Skeleton className="h-6 w-1/3" />
        <Skeleton className="mt-3 h-4 w-2/3" />
      </Card>
    );
  }
  if (allowance.isError) {
    return (
      <Alert role="alert" tone="danger" title={t("mqttAllowance.project.errorTitle")} data-allowance="error">
        {allowance.error instanceof Error ? forPeople(allowance.error.message) : t("mqttAllowance.project.error")}
      </Alert>
    );
  }
  const data = allowance.data;
  if (!data.enabled) {
    return (
      <Alert tone="info" title={t("mqttAllowance.project.offTitle")} data-allowance="off">
        {t("mqttAllowance.project.off", { project })}
      </Alert>
    );
  }
  const number = new Intl.NumberFormat(i18n.resolvedLanguage ?? i18n.language ?? "en");
  const full = data.brokers.left === 0;
  return (
    <Card>
      <section aria-labelledby="mqtt-allowance" className="flex flex-col gap-3" data-allowance="on">
        <h2 id="mqtt-allowance" className="text-title font-semibold text-fg">
          {t("mqttAllowance.project.title")}
        </h2>
        <p className="text-body text-fg" data-allowance-left="">
          {t("mqttAllowance.project.left", {
            used: data.brokers.used,
            limit: data.brokers.limit,
            connections: number.format(data.connections.left),
            connectionsLimit: number.format(data.connections.limit),
            rate: number.format(data.messagesPerSecond.left),
          })}
        </p>
        <ul className="flex flex-wrap gap-2" aria-label={t("mqttAllowance.project.modes")}>
          {data.modes.map((mode) => (
            <li key={mode}>
              <Badge tone="neutral">{t(`mqttAllowance.modeLong.${mode}`, { defaultValue: mode })}</Badge>
            </li>
          ))}
        </ul>
        <p className="text-caption text-fg-muted">
          {t("mqttAllowance.project.addresses", {
            addresses: data.addresses.map((address) => t(`mqttAllowance.address.${address}`, { defaultValue: address })).join(", "),
          })}
          {data.ports.length > 0 ? ` ${t("mqttAllowance.project.ports", { ports: data.ports.join(", ") })}` : ""}
        </p>
        {full ? (
          <Alert tone="warning" data-allowance="full">
            {t("mqttAllowance.project.full", { limit: data.brokers.limit })}
          </Alert>
        ) : null}
      </section>
    </Card>
  );
}

/** Device → broker → data source → pipeline → Endpoint → Policy, one row per edge. */
export function Connections({ project, brokers, bare }: { project: string; brokers: Manifest[]; bare?: boolean }): JSX.Element {
  const { t } = useTranslation();
  const dataSources = useReadableList(project, "datasources", "DataSource");
  const pipelines = useReadableList(project, "pipelines", "Pipeline");
  const endpoints = useReadableList(project, "endpoints", "Endpoint");
  const policies = useReadableList(project, "policies", "Policy");
  const accounts = useReadableList(project, "serviceaccounts", "ServiceAccount");
  const lists = [dataSources, pipelines, endpoints, policies, accounts];
  const of = (query: (typeof lists)[number]) => asManifests(query.data?.items ?? []);
  const edges = lists.some((query) => query.isPending)
    ? []
    : connectionsOf({
        project,
        brokers,
        dataSources: of(dataSources),
        pipelines: of(pipelines),
        endpoints: of(endpoints),
        policies: of(policies),
        accounts: of(accounts),
      });
  const hidden = lists.some((query) => query.hidden);
  const table = (
    <Table stack caption={t("mqttbrokers.connections.title")}>
      <TableHead>
        <TableHeaderCell>{t("mqttbrokers.connections.edge")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.connections.from")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.connections.to")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.connections.allowedBy")}</TableHeaderCell>
        <TableHeaderCell>{t("mqttbrokers.connections.state")}</TableHeaderCell>
      </TableHead>
      <TableBody>
        {edges.map((edge, index) => (
          <EdgeRow key={`${edge.kind}-${edge.from}-${edge.to}-${index}`} project={project} edge={edge} />
        ))}
      </TableBody>
    </Table>
  );
  // On a broker's own page the section around it carries the title and the lead.
  if (bare) return table;
  return (
    <section aria-labelledby="mqtt-connections" className="flex flex-col gap-3">
      <h2 id="mqtt-connections" className="text-title font-semibold text-fg">
        {t("mqttbrokers.connections.title")}
      </h2>
      <p className="text-body text-fg-muted">
        {t("mqttbrokers.connections.lead")}
        {hidden ? ` ${t("mqttbrokers.connections.partial")}` : ""}
      </p>
      {table}
    </section>
  );
}

function EdgeRow({ project, edge }: { project: string; edge: Edge }): JSX.Element {
  const { t } = useTranslation();
  return (
    <TableRow data-edge={edge.problem ? "red" : "green"}>
      <TableCell>{t(`mqttbrokers.connections.kind.${edge.kind}`)}</TableCell>
      <TableCell primary data-label={t("mqttbrokers.connections.from")}>{edge.from}</TableCell>
      <TableCell data-label={t("mqttbrokers.connections.to")}>{edge.to || "—"}</TableCell>
      <TableCell data-label={t("mqttbrokers.connections.allowedBy")}>{t(edge.allowedBy.key, edge.allowedBy.values)}</TableCell>
      <TableCell data-label={t("mqttbrokers.connections.state")}>
        {edge.problem ? (
          <span className="flex flex-col gap-1">
            <Badge tone="danger">{t(edge.problem.key, edge.problem.values)}</Badge>
            {edge.fix ? (
              edge.fix.name ? (
                <RecordLink project={project} plural={edge.fix.plural} name={edge.fix.name}>
                  {t("mqttbrokers.connections.fix")}
                </RecordLink>
              ) : (
                <Link
                  to="/projects/$project/$plural"
                  params={{ project, plural: edge.fix.plural }}
                  className={RECORD_LINK_STYLE}
                >
                  {t("mqttbrokers.connections.fix")}
                </Link>
              )
            ) : null}
          </span>
        ) : (
          <Badge tone="success">{t("mqttbrokers.connections.ok")}</Badge>
        )}
      </TableCell>
    </TableRow>
  );
}
