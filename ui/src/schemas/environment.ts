import type { JsonSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DIGEST_PATTERN } from "./agentprofile";
import { DNS1123 } from "./kinds";

/**
 * The Environment form (T-1545, CC-73…CC-75): the values one environment renders the repository
 * with, written by an administrator on the Organization page.
 *
 * The manifest holds hosts, images and feature flags as maps; the form holds them as rows of a name
 * and a value, so a person adds one without typing YAML. An overlay names the secret backend, never a
 * secret: the form has no field a value could be typed into (CC-75).
 */

/** A lowercase domain name of at least two labels, without a scheme or a path. */
export const DOMAIN_PATTERN = "^[a-z0-9]([-a-z0-9]*[a-z0-9])?(\\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)+$";

/** A component or flag name as the overlay keys it: letters, digits and hyphens. */
const KEY_PATTERN = "^[A-Za-z0-9][-A-Za-z0-9]*$";

export interface EnvironmentForm {
  name?: string;
  orgDomain?: string;
  hosts?: { component?: string; host?: string }[];
  images?: { component?: string; digest?: string }[];
  secrets?: { backend?: string; mount?: string };
  features?: { name?: string; enabled?: boolean }[];
}

export function environmentSchema(t: (key: string) => string): JsonSchema {
  const text = (key: string, pattern: string): JsonSchema => ({
    type: "string",
    title: t(`environments.field.${key}`),
    pattern,
  });
  const rows = (key: string, value: JsonSchema, first = "component"): JsonSchema => ({
    type: "array",
    title: t(`environments.field.${key}`),
    items: {
      type: "object",
      required: [first, Object.keys(value)[0]],
      properties: { [first]: text(first, KEY_PATTERN), ...value },
    },
  });
  return {
    type: "object",
    required: ["name"],
    properties: {
      name: { ...text("name", DNS1123), maxLength: 63 },
      orgDomain: text("orgDomain", DOMAIN_PATTERN),
      hosts: rows("hosts", { host: text("host", DOMAIN_PATTERN) }),
      // CC-74: a tag moves and a digest does not.
      images: rows("images", { digest: text("digest", DIGEST_PATTERN) }),
      secrets: {
        type: "object",
        title: t("environments.field.secrets"),
        properties: { backend: text("backend", "\\S"), mount: text("mount", "\\S") },
      },
      features: {
        type: "array",
        title: t("environments.field.features"),
        items: {
          type: "object",
          required: ["name"],
          properties: {
            name: text("feature", KEY_PATTERN),
            enabled: { type: "boolean", title: t("environments.field.enabled"), default: true },
          },
        },
      },
    },
  };
}

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** Rows as the map the manifest holds; a row without its name or its value is dropped. */
function map<V>(entries: [string | undefined, V | undefined][]): Record<string, V> {
  return Object.fromEntries(
    entries.flatMap(([key, value]) => {
      const name = filled(key);
      return name && value !== undefined ? [[name, value]] : [];
    }),
  );
}

const some = (value: object): boolean => Object.keys(value).length > 0;

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toEnvironmentManifest(form: EnvironmentForm, stored?: unknown): unknown {
  const orgDomain = filled(form.orgDomain);
  const hosts = map((form.hosts ?? []).map((row) => [row.component, filled(row.host)]));
  const images = map((form.images ?? []).map((row) => [row.component, filled(row.digest)]));
  const features = map((form.features ?? []).map((row) => [row.name, row.enabled ?? true]));
  const backend = filled(form.secrets?.backend);
  const mount = filled(form.secrets?.mount);
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Environment",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: "org" },
    spec: {
      ...(orgDomain ? { orgDomain } : {}),
      ...(some(hosts) ? { hosts } : {}),
      ...(some(images) ? { images } : {}),
      // A mount without its backend is refused by jc-core naming `spec.secrets.backend`.
      ...(backend || mount ? { secrets: { backend: backend ?? "", ...(mount ? { mount } : {}) } } : {}),
      ...(some(features) ? { features } : {}),
    },
  };
}

/** The stored manifest back as the form, each map as rows. */
export function fromEnvironmentManifest(manifest: unknown): EnvironmentForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const entries = (value: unknown): [string, unknown][] =>
    value && typeof value === "object" ? Object.entries(value as Record<string, unknown>) : [];
  const secrets = spec.secrets as { backend?: string; mount?: string } | undefined;
  return {
    name: envelope.metadata?.name ?? "",
    ...(typeof spec.orgDomain === "string" ? { orgDomain: spec.orgDomain } : {}),
    hosts: entries(spec.hosts).map(([component, host]) => ({ component, host: String(host) })),
    images: entries(spec.images).map(([component, digest]) => ({ component, digest: String(digest) })),
    ...(secrets ? { secrets } : {}),
    features: entries(spec.features).map(([name, enabled]) => ({ name, enabled: enabled === true })),
  };
}
