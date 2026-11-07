import { useQuery } from "@tanstack/react-query";
import { api, unwrap } from "../../api/client";
import type { JsonSchema } from "../../components/forms/types";
import { DEDICATED_PORTS } from "../../schemas/mqttbroker";

/**
 * A project's MQTT allowance (T-4860, PF-117…PF-119, ADR-N-048 §2.9): what an org admin lets the
 * project run as edge brokers. It is the Project's `spec.quotas.mqtt` or the Organization's
 * `spec.limits.mqtt`, written through a Change only an org admin may propose; the API answers 403
 * to anybody else, so this form is a convenience, never the control.
 */

/** The broker modes, in the order a person reads them: each one adds to the one before. */
export const MQTT_MODES = ["ingest", "capture", "relay"] as const;
/** How a device reaches a broker from outside (ADR-N-048 §2.11). */
export const MQTT_ADDRESSES = ["platform", "dedicatedPort"] as const;

/** jc-core's bounds of `spec.limits.mqtt.*` (org_settings.rs), the operator's ceiling. */
export const ALLOWANCE_BOUNDS = {
  brokers: 20,
  connections: 20_000,
  messagesPerSecond: 100_000,
  maxMessageBytes: 1024 * 1024,
} as const;

export interface AllowanceForm {
  enabled?: boolean;
  brokers?: number;
  connections?: number;
  messagesPerSecond?: number;
  maxMessageBytes?: number;
  modes?: string[];
  addresses?: string[];
  ports?: number[];
}

type T = (key: string) => string;

/** The allowance's fields, for the per-project card and the organization's default alike. */
export function allowanceProperties(t: T): Record<string, JsonSchema> {
  const title = (key: string) => t(`mqttAllowance.field.${key}`);
  const hint = (key: string) => t(`mqttAllowance.field.${key}Hint`);
  const number = (key: keyof typeof ALLOWANCE_BOUNDS, min = 0): JsonSchema => ({
    type: "integer",
    title: title(key),
    description: hint(key),
    minimum: min,
    maximum: ALLOWANCE_BOUNDS[key],
  });
  return {
    enabled: { type: "boolean", title: title("enabled"), description: hint("enabled") },
    brokers: number("brokers"),
    connections: number("connections"),
    messagesPerSecond: number("messagesPerSecond"),
    maxMessageBytes: number("maxMessageBytes", 1),
    modes: {
      type: "array",
      title: title("modes"),
      description: hint("modes"),
      uniqueItems: true,
      items: { type: "string", oneOf: MQTT_MODES.map((mode) => ({ const: mode, title: t(`mqttAllowance.mode.${mode}`) })) },
    },
    addresses: {
      type: "array",
      title: title("addresses"),
      description: hint("addresses"),
      uniqueItems: true,
      items: {
        type: "string",
        oneOf: MQTT_ADDRESSES.map((address) => ({ const: address, title: t(`mqttAllowance.address.${address}`) })),
      },
    },
    ports: {
      type: "array",
      title: title("ports"),
      description: hint("ports"),
      uniqueItems: true,
      items: { type: "integer", minimum: DEDICATED_PORTS.min, maximum: DEDICATED_PORTS.max },
    },
  };
}

export function allowanceSchema(t: T): JsonSchema {
  return { type: "object", properties: allowanceProperties(t) };
}

/** The Project manifest's `spec.quotas.mqtt` as the form. */
export function fromProjectAllowance(manifest: unknown): Record<string, unknown> {
  const spec = ((manifest ?? {}) as { spec?: { quotas?: { mqtt?: AllowanceForm } } }).spec;
  return { ...(spec?.quotas?.mqtt ?? { enabled: false }) };
}

/**
 * The stored Project with the form as its `spec.quotas.mqtt`; every other field travels on as it
 * was (T-2470), `status` never goes back (MF-04). An empty field is dropped, so the catalog
 * default applies, and an empty list is dropped for the same reason: `modes: []` would be read
 * as "no mode" by nobody, and absent is `[ingest]`.
 */
export function toProjectAllowance(form: Record<string, unknown>, stored: unknown): unknown {
  const manifest = { ...((stored ?? {}) as Record<string, unknown>) };
  delete manifest.status;
  const spec = { ...((manifest.spec ?? {}) as Record<string, unknown>) };
  const quotas = { ...((spec.quotas ?? {}) as Record<string, unknown>) };
  const mqtt: Record<string, unknown> = { enabled: form.enabled === true };
  for (const [key, value] of Object.entries(form)) {
    if (key === "enabled") continue;
    if (typeof value === "number") mqtt[key] = value;
    if (Array.isArray(value)) {
      const items = value.filter((item) => item !== undefined && item !== null && item !== "");
      if (items.length > 0) mqtt[key] = items;
    }
  }
  quotas.mqtt = mqtt;
  spec.quotas = quotas;
  return { ...manifest, spec };
}

/** `GET /api/v1/projects/{project}/mqtt/allowance` (API/01 §47): whoever may read the project. */
export function useMqttAllowance(project: string) {
  return useQuery({
    queryKey: ["mqttAllowance", project],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/mqtt/allowance", {
          params: { path: { project } },
        }),
      ),
  });
}

/** The boxes, not a multi-select, for the two lists. */
export const ALLOWANCE_UI = {
  modes: { "ui:widget": "checkboxes" },
  addresses: { "ui:widget": "checkboxes" },
} as const;

/**
 * The Organization form with the MQTT default every project without its own allowance runs on:
 * the catalog gives `spec.limits.mqtt`'s numbers, the switch and the two lists are no number.
 * Ports are each project's own and never offered here.
 */
export function withMqttDefault(schema: JsonSchema, t: T): JsonSchema {
  const root = structuredClone(schema) as { properties?: Record<string, { properties?: Record<string, unknown> }> };
  const mqtt = root.properties?.limits?.properties?.mqtt as { properties?: Record<string, JsonSchema> } | undefined;
  if (mqtt) {
    const { enabled, modes, addresses } = allowanceProperties(t);
    mqtt.properties = { enabled, ...mqtt.properties, modes, addresses };
  }
  return root as JsonSchema;
}
