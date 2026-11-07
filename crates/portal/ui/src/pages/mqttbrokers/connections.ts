import type { Manifest } from "../../api/manifest";
import { holdersOf } from "./brokerDraft";

/**
 * Who talks to whom around a project's MQTT brokers (T-3839, ADR-N-048): device → broker → data
 * source → pipeline → Endpoint → Policy, each edge with the identity or the grant that allows it.
 *
 * An edge is red when the manifests say it cannot work: a role naming a ServiceAccount the project
 * does not declare, a broker that is missing or in error, a data source no pipeline reads, an
 * Endpoint missing from the project, or a Policy it names that is not there. What only a running system knows (an
 * expired client, a dropped connection) is not read here.
 */

export type EdgeKind = "device" | "broker" | "pipeline" | "endpoint" | "policy";

/** The resource a red edge's fix is on. */
export interface FixAt {
  plural: string;
  name: string;
}

export interface Edge {
  kind: EdgeKind;
  /** The two ends, by name. */
  from: string;
  to: string;
  /** The identity or the grant that allows the edge; a label key with its values. */
  allowedBy: { key: string; values?: Record<string, string> };
  /** Why the edge cannot work, as a label key with its values; absent on a green edge. */
  problem?: { key: string; values?: Record<string, string> };
  fix?: FixAt;
}

export interface Resources {
  project: string;
  brokers: Manifest[];
  dataSources: Manifest[];
  pipelines: Manifest[];
  endpoints: Manifest[];
  policies: Manifest[];
  accounts: Manifest[];
}

type Spec = Record<string, unknown>;

/** A `Ref`: a bare name or `{kind, name}`. */
function refName(value: unknown): string | undefined {
  if (typeof value === "string" && value !== "") return value;
  const name = (value as { name?: unknown } | null | undefined)?.name;
  return typeof name === "string" && name !== "" ? name : undefined;
}

/**
 * The resource name an Endpoint or Policy URN ends in. Nothing else is read out of it: a URN's
 * other segments name no space, organization or project (ADR-N-041).
 */
export function urnName(urn: unknown): string | undefined {
  if (typeof urn !== "string") return undefined;
  const parts = urn.split(":");
  if (parts.length < 4 || parts[0] !== "urn" || parts[1] !== "ngsi-ld") return undefined;
  return parts[parts.length - 1] || undefined;
}

/** The DataSources a pipeline reads, by name, from `source` and every entry of `sources`. */
export function readsOf(spec: Spec): string[] {
  const sources = [spec.source, ...(Array.isArray(spec.sources) ? spec.sources : [])] as Array<Spec | undefined>;
  return sources.map((source) => refName(source?.dataSourceRef)).filter((name): name is string => !!name);
}

function writesOf(spec: Spec): unknown[] {
  const outputs = Array.isArray(spec.outputs) ? (spec.outputs as Spec[]) : [];
  return [spec.targetEndpoint, ...outputs.map((output) => output.targetEndpoint)].filter((urn) => urn !== undefined);
}

function phaseOf(item: Manifest): string {
  return String((item.status as { phase?: unknown } | undefined)?.phase ?? "").toLowerCase();
}

/** Every edge around the project's brokers, in the order a message travels. */
export function connectionsOf(resources: Resources): Edge[] {
  const { project } = resources;
  const named = (items: Manifest[]) => new Map(items.map((item) => [item.metadata.name, item]));
  const brokers = named(resources.brokers);
  const endpoints = named(resources.endpoints);
  const policies = named(resources.policies);
  const accounts = new Set(resources.accounts.map((account) => account.metadata.name));
  const edges: Edge[] = [];

  for (const broker of resources.brokers) {
    const spec = (broker.spec ?? {}) as { roles?: Array<{ name?: string; serviceAccounts?: unknown; publish?: unknown }> };
    for (const role of spec.roles ?? []) {
      const listed = Array.isArray(role.serviceAccounts) ? role.serviceAccounts.filter((a): a is string => typeof a === "string") : [];
      // The role's own list (deprecated) and every account a RoleBinding grants it (T-5872, T-6103).
      const members = [...new Set(holdersOf({ name: role.name, serviceAccounts: listed }, resources.accounts, broker.metadata.name, project).map((h) => h.account))];
      for (const account of members) {
        edges.push({
          kind: "device",
          from: account,
          to: broker.metadata.name,
          allowedBy: { key: "mqttbrokers.connections.byRole", values: { role: role.name ?? "", client: `${project}-${account}` } },
          ...(accounts.has(account)
            ? {}
            : {
                problem: { key: "mqttbrokers.connections.unknownAccount", values: { account } },
                fix: { plural: "mqttbrokers", name: broker.metadata.name },
              }),
        });
      }
    }
  }

  const readers = new Map<string, string[]>();
  for (const pipeline of resources.pipelines) {
    for (const source of readsOf((pipeline.spec ?? {}) as Spec)) {
      readers.set(source, [...(readers.get(source) ?? []), pipeline.metadata.name]);
    }
  }

  for (const source of resources.dataSources) {
    const mqtt = ((source.spec ?? {}) as { mqtt?: { brokerRef?: unknown } }).mqtt;
    const brokerName = refName(mqtt?.brokerRef);
    if (!brokerName) continue;
    const broker = brokers.get(brokerName);
    edges.push({
      kind: "broker",
      from: brokerName,
      to: source.metadata.name,
      allowedBy: { key: "mqttbrokers.connections.byListener" },
      ...(!broker
        ? {
            problem: { key: "mqttbrokers.connections.noBroker", values: { broker: brokerName } },
            fix: { plural: "datasources", name: source.metadata.name },
          }
        : phaseOf(broker) === "error"
          ? {
              problem: { key: "mqttbrokers.connections.brokerError", values: { broker: brokerName } },
              fix: { plural: "mqttbrokers", name: brokerName },
            }
          : {}),
    });

    const reading = readers.get(source.metadata.name) ?? [];
    if (reading.length === 0) {
      edges.push({
        kind: "pipeline",
        from: source.metadata.name,
        to: "",
        allowedBy: { key: "mqttbrokers.connections.byRunner" },
        problem: { key: "mqttbrokers.connections.unread" },
        fix: { plural: "pipelines", name: "" },
      });
    }
    for (const pipelineName of reading) {
      edges.push({
        kind: "pipeline",
        from: source.metadata.name,
        to: pipelineName,
        allowedBy: { key: "mqttbrokers.connections.byRunner" },
      });
      const pipeline = resources.pipelines.find((item) => item.metadata.name === pipelineName);
      for (const urn of writesOf((pipeline?.spec ?? {}) as Spec)) {
        const target = urnName(urn) ?? String(urn);
        const endpoint = endpoints.get(target);
        edges.push({
          kind: "endpoint",
          from: pipelineName,
          to: target,
          allowedBy: { key: "mqttbrokers.connections.byPipelineAccount", values: { pipeline: pipelineName } },
          ...(endpoint
            ? {}
            : {
                problem: { key: "mqttbrokers.connections.noEndpoint", values: { endpoint: target } },
                fix: { plural: "pipelines", name: pipelineName },
              }),
        });
        if (!endpoint) continue;
        // An Endpoint without a policyRef is guarded by its space's grants, which this table does
        // not judge; one naming a Policy the project lacks is red.
        const policy = urnName((endpoint.spec as Spec | undefined)?.policyRef);
        if (!policy) continue;
        edges.push({
          kind: "policy",
          from: endpoint.metadata.name,
          to: policy,
          allowedBy: { key: "mqttbrokers.connections.byPolicy" },
          ...(policies.has(policy)
            ? {}
            : {
                problem: { key: "mqttbrokers.connections.noPolicy", values: { policy } },
                fix: { plural: "endpoints", name: endpoint.metadata.name },
              }),
        });
      }
    }
  }
  return edges;
}
