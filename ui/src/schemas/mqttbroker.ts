import type { JsonSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123 } from "./kinds";

/**
 * The MqttBroker form (T-3839, MF-54, ADR-N-048): the roles that name which ServiceAccounts may
 * publish or subscribe to which topics, the broker's limits and an optional dedicated port.
 *
 * The bounds are jc-core's (`kinds/mqtt_broker.rs`), so a manifest the API would refuse is caught at
 * the field; the API still decides. A broker holds no credential of its own: a device signs in with
 * its ServiceAccount's token and a pipeline reads on the internal listener.
 */

/** The dedicated ports a broker may ask for (`DEDICATED_PORTS`). */
export const DEDICATED_PORTS = { min: 18830, max: 18849 } as const;

/**
 * One network of `allowedSources` (PF-129): an IPv4 or IPv6 address and a prefix. The field catches
 * a typo; jc-core still refuses host bits, a hostname and every-address ranges with the field named.
 */
export const SOURCE_RANGE =
  "^((25[0-5]|2[0-4]\\d|1?\\d?\\d)(\\.(25[0-5]|2[0-4]\\d|1?\\d?\\d)){3}/(3[0-2]|[12]?\\d)|[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]*/(12[0-8]|1[01]\\d|[1-9]?\\d))$";

/** The most networks a broker may list (`MAX_ALLOWED_SOURCES`). */
export const MAX_ALLOWED_SOURCES = 32;

/** A device subscribes under its own client id only, with no wildcard before it. */
export const DEVICE_SUBSCRIBE = "^[^+#]*\\$\\{clientid\\}";

export interface MqttRoleForm {
  name?: string;
  serviceAccounts?: string[];
  publish?: string[];
  subscribe?: string[];
  /** The role's devices share their publish topics (T-5561); needs a `reason`. */
  sharedTopics?: boolean;
  reason?: string;
}

export interface MqttBrokerForm {
  name?: string;
  roles?: MqttRoleForm[];
  limits?: {
    maxMessageBytes?: number;
    messagesPerSecond?: number;
    maxConnections?: number;
    connectionsPerAddress?: number;
  };
  port?: number;
  allowedSources?: string[];
  mode?: string;
  capture?: { messagesPerTopic?: number; window?: string };
}

/** The modes in plain words, each adding to the one before (T-4860, ADR-N-048 §2.10). */
export const BROKER_MODES = ["ingest", "capture", "relay"] as const;

/** A capture window: an ISO 8601 duration of at most a day, as jc-core reads it. */
export const CAPTURE_WINDOW = "^PT(?=\\d)(\\d+H)?(\\d+M)?(\\d+S)?$";

/** The windows the broker form offers, at most a day (jc-core's ceiling). */
export const CAPTURE_WINDOWS = ["PT15M", "PT30M", "PT1H", "PT2H", "PT6H", "PT12H", "PT24H"] as const;

/**
 * An ISO 8601 time duration in words of the language ("2 hours", "1 hour 30 minutes"), so no person
 * reads `PT2H`. A value that is not PT…H…M…S comes back as it was.
 */
export function durationWords(iso: string, language: string): string {
  const match = new RegExp(CAPTURE_WINDOW).exec(iso);
  if (!match) return iso;
  const units = ["hour", "minute", "second"] as const;
  return units
    .flatMap((unit, index) => {
      const amount = Number.parseInt(match[index + 1] ?? "", 10);
      return Number.isNaN(amount) ? [] : [new Intl.NumberFormat(language, { style: "unit", unit, unitDisplay: "long" }).format(amount)];
    })
    .join(" ");
}

export function mqttBrokerSchema(t: (key: string) => string, language = "en"): JsonSchema {
  const title = (key: string) => t(`mqttbrokers.field.${key}`);
  const filters = (key: string, pattern?: string): JsonSchema => ({
    type: "array",
    title: title(key),
    items: { type: "string", minLength: 1, maxLength: 256, ...(pattern ? { pattern } : {}) },
  });
  const limit = (key: string, max: number): JsonSchema => ({
    type: "integer",
    title: title(key),
    minimum: 1,
    maximum: max,
  });
  return {
    type: "object",
    required: ["name", "roles"],
    properties: {
      name: { type: "string", title: title("name"), pattern: DNS1123, maxLength: 63 },
      mode: {
        type: "string",
        title: title("mode"),
        default: "ingest",
        oneOf: BROKER_MODES.map((mode) => ({ const: mode, title: t(`mqttAllowance.modeLong.${mode}`) })),
      },
      capture: {
        type: "object",
        title: title("capture"),
        properties: {
          messagesPerTopic: { type: "integer", title: title("messagesPerTopic"), minimum: 1, maximum: 1000 },
          window: {
            type: "string",
            title: title("window"),
            oneOf: CAPTURE_WINDOWS.map((window) => ({ const: window, title: durationWords(window, language) })),
          },
        },
      },
      roles: {
        type: "array",
        title: title("roles"),
        minItems: 1,
        items: {
          type: "object",
          required: ["name", "serviceAccounts"],
          properties: {
            name: { type: "string", title: title("roleName"), pattern: DNS1123, maxLength: 63 },
            serviceAccounts: {
              type: "array",
              title: title("serviceAccounts"),
              minItems: 1,
              items: { type: "string", pattern: DNS1123, maxLength: 63 },
            },
            publish: filters("publish"),
            subscribe: filters("subscribe", DEVICE_SUBSCRIBE),
          },
        },
      },
      limits: {
        type: "object",
        title: title("limits"),
        properties: {
          maxMessageBytes: limit("maxMessageBytes", 1024 * 1024),
          messagesPerSecond: limit("messagesPerSecond", 1000),
          maxConnections: limit("maxConnections", 10_000),
          // Traefik closes a connection over this count from one address (T-5888); jc-core holds
          // it to maxConnections, which one field cannot compare.
          connectionsPerAddress: limit("connectionsPerAddress", 10_000),
        },
      },
      port: { type: "integer", title: title("port"), minimum: DEDICATED_PORTS.min, maximum: DEDICATED_PORTS.max },
      allowedSources: {
        type: "array",
        title: title("allowedSources"),
        maxItems: MAX_ALLOWED_SOURCES,
        items: { type: "string", minLength: 1, maxLength: 43, pattern: SOURCE_RANGE },
      },
    },
  };
}

/** The form as the manifest the API stores; the metadata the form has no field for travels on. */
export function toMqttBrokerManifest(project: string, form: MqttBrokerForm, stored?: unknown): unknown {
  // A row the form added and nobody filled is undefined, not "".
  const list = (values?: (string | undefined)[]) =>
    (values ?? []).map((value) => (value ?? "").trim()).filter(Boolean);
  const limits = Object.fromEntries(
    Object.entries(form.limits ?? {}).filter(([, value]) => typeof value === "number"),
  );
  // Capture settings only travel with the capture mode, which is the only one jc-core keeps them for.
  const capture = Object.fromEntries(
    Object.entries(form.capture ?? {}).filter(([, value]) => value !== undefined && value !== ""),
  );
  // The settings the form has no field for (address, hostname, deviceAuth) travel on unchanged
  // (T-6103: an edit used to drop them).
  const storedSpec = ((stored ?? {}) as { spec?: Record<string, unknown> }).spec ?? {};
  const kept = Object.fromEntries(
    Object.entries(storedSpec).filter(([key]) => ["address", "hostname", "deviceAuth"].includes(key)),
  );
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "MqttBroker",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: project },
    spec: {
      ...kept,
      roles: (form.roles ?? []).map((role) => {
        const publish = list(role.publish);
        const subscribe = list(role.subscribe);
        const reason = (role.reason ?? "").trim();
        return {
          name: role.name ?? "",
          serviceAccounts: list(role.serviceAccounts),
          ...(publish.length > 0 ? { publish } : {}),
          ...(subscribe.length > 0 ? { subscribe } : {}),
          ...(role.sharedTopics ? { sharedTopics: true } : {}),
          ...(role.sharedTopics && reason ? { reason } : {}),
        };
      }),
      ...(Object.keys(limits).length > 0 ? { limits } : {}),
      ...(typeof form.port === "number" ? { port: form.port } : {}),
      ...(list(form.allowedSources).length > 0 ? { allowedSources: list(form.allowedSources) } : {}),
      ...(form.mode && form.mode !== "ingest" ? { mode: form.mode } : {}),
      ...(form.mode === "capture" && Object.keys(capture).length > 0 ? { capture } : {}),
    },
  };
}

/** The stored manifest back as the form. */
export function fromMqttBrokerManifest(manifest: unknown): MqttBrokerForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Partial<MqttBrokerForm> };
  const spec = envelope.spec ?? {};
  return {
    name: envelope.metadata?.name ?? "",
    roles: (spec.roles ?? []).map((role) => ({
      name: role.name ?? "",
      serviceAccounts: role.serviceAccounts ?? [],
      publish: role.publish ?? [],
      subscribe: role.subscribe ?? [],
      ...(role.sharedTopics ? { sharedTopics: true, reason: role.reason ?? "" } : {}),
    })),
    ...(spec.limits ? { limits: spec.limits } : {}),
    ...(typeof spec.port === "number" ? { port: spec.port } : {}),
    ...(spec.allowedSources?.length ? { allowedSources: spec.allowedSources } : {}),
    mode: spec.mode ?? "ingest",
    ...(spec.capture ? { capture: spec.capture } : {}),
  };
}
