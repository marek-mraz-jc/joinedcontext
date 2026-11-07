import type { JsonSchema, UiSchema } from "../components/forms/types";
import { storedMetadata } from "../api/manifest";
import { DNS1123, words } from "./kinds";

/**
 * The AgentProfile form (T-1536, AG-26, AG-41, AG-47…AG-50, AG-70, MF-40): what an agent run of the
 * organization executes, calls and may reach, written by an administrator on the Organization page.
 *
 * jc-core decides what a valid profile is and names the field it refuses: a steward profile with
 * tools or hosts, a builder without its allow-list, a wall clock past an hour, an operation the
 * Portal does not register. The form holds each block as the manifest does, and has no field a model
 * key could be typed into: the proxy holds those, never the profile.
 */

export const PROFILE_ROLES = ["steward", "builder"] as const;
export const MODEL_PROVIDERS = ["openai-compatible", "anthropic"] as const;
export const REASONING_EFFORTS = ["low", "medium", "high"] as const;
export const AGENT_TOOLS = ["shell", "cargo", "pnpm", "git", "playwright"] as const;
export const KIND_VERBS = ["read", "propose"] as const;
export const ENDPOINT_VERBS = ["read", "write"] as const;

/** `sha256:` and 64 lowercase hex digits (AG-49). */
export const DIGEST_PATTERN = "^sha256:[0-9a-f]{64}$";
/** `PT[#H][#M][#S]`, as jc-core's `parse_iso_duration` reads it; at most an hour is its check. */
export const WALL_CLOCK_PATTERN = "^PT(?=[0-9])([0-9]+H)?([0-9]+M)?([0-9]+S)?$";
/** A bare hostname: no scheme, port, path or wildcard (AG-50). */
export const HOST_PATTERN =
  "^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$";
/** A registry operation name (MF-40). */
export const OPERATION_PATTERN = "^jc_[a-z0-9_]+$";

export interface AgentProfileForm {
  name?: string;
  role?: string;
  runtime?: { image?: string; digest?: string };
  model?: { provider?: string; name?: string; maxTokensPerRun?: number; reasoningEffort?: string };
  limits?: {
    stepsPerRun?: number;
    wallClock?: string;
    concurrentRunsPerOrganization?: number;
    requestsPerMinute?: number;
    maxResponseBytes?: number;
  };
  egress?: { allowedHosts?: string[]; maxBytesPerRun?: number };
  tools?: string[];
  workspace?: { cpu?: string; memory?: string; ephemeralStorage?: string };
  access?: {
    operations?: string[];
    kinds?: { kind?: string; verbs?: string[] }[];
    endpoints?: { name?: string; verbs?: string[] }[];
  };
}

export function agentProfileSchema(t: (key: string) => string): JsonSchema {
  const title = (key: string): string => t(`agentprofiles.field.${key}`);
  const text = (key: string, pattern = "\\S"): JsonSchema => ({ type: "string", title: title(key), pattern });
  const count = (key: string, fallback?: number): JsonSchema => ({
    type: "integer",
    title: title(key),
    minimum: 1,
    ...(fallback !== undefined ? { default: fallback } : {}),
  });
  const verbs = (values: readonly string[]): JsonSchema => ({
    type: "array",
    title: title("verbs"),
    minItems: 1,
    uniqueItems: true,
    items: { type: "string", ...words(t, "choice.agentVerb", values) },
  });
  return {
    type: "object",
    required: ["name", "role", "runtime", "model", "limits", "workspace"],
    properties: {
      name: { ...text("name", DNS1123), maxLength: 63 },
      // AG-26: a steward reaches the Portal's operations and nothing on the internet.
      role: { type: "string", title: title("role"), ...words(t, "choice.profileRole", PROFILE_ROLES), default: "steward" },
      runtime: {
        type: "object",
        title: title("runtime"),
        required: ["image", "digest"],
        properties: { image: text("image"), digest: text("digest", DIGEST_PATTERN) },
      },
      model: {
        type: "object",
        title: title("model"),
        required: ["provider", "name", "maxTokensPerRun"],
        properties: {
          provider: {
            type: "string",
            title: title("provider"),
            ...words(t, "choice.modelProvider", MODEL_PROVIDERS),
            default: "openai-compatible",
          },
          name: text("modelName"),
          maxTokensPerRun: count("maxTokensPerRun"),
          reasoningEffort: { type: "string", title: title("reasoningEffort"), ...words(t, "choice.reasoningEffort", REASONING_EFFORTS) },
        },
      },
      // AG-41: the bounds every run is held to; the defaults are the reference profile's.
      limits: {
        type: "object",
        title: title("limits"),
        required: ["stepsPerRun", "wallClock", "concurrentRunsPerOrganization", "requestsPerMinute", "maxResponseBytes"],
        properties: {
          stepsPerRun: count("stepsPerRun", 120),
          wallClock: { ...text("wallClock", WALL_CLOCK_PATTERN), default: "PT20M" },
          concurrentRunsPerOrganization: count("concurrentRuns", 2),
          requestsPerMinute: count("requestsPerMinute", 60),
          maxResponseBytes: count("maxResponseBytes", 2097152),
        },
      },
      egress: {
        type: "object",
        title: title("egress"),
        properties: {
          allowedHosts: { type: "array", title: title("allowedHosts"), uniqueItems: true, items: text("host", HOST_PATTERN) },
          maxBytesPerRun: count("maxBytesPerRun"),
        },
      },
      tools: {
        type: "array",
        title: title("tools"),
        uniqueItems: true,
        items: { type: "string", ...words(t, "choice.agentTool", AGENT_TOOLS) },
      },
      workspace: {
        type: "object",
        title: title("workspace"),
        required: ["cpu", "memory", "ephemeralStorage"],
        properties: {
          cpu: { ...text("cpu"), default: "1" },
          memory: { ...text("memory"), default: "2Gi" },
          ephemeralStorage: { ...text("ephemeralStorage"), default: "4Gi" },
        },
      },
      // AG-70: the most a run may reach; the starting person's own grants narrow every call.
      access: {
        type: "object",
        title: title("access"),
        properties: {
          operations: {
            type: "array",
            title: title("operations"),
            uniqueItems: true,
            items: text("operation", OPERATION_PATTERN),
          },
          kinds: {
            type: "array",
            title: title("kinds"),
            items: {
              type: "object",
              required: ["kind", "verbs"],
              properties: { kind: text("kind", "^[A-Z][A-Za-z]*$"), verbs: verbs(KIND_VERBS) },
            },
          },
          endpoints: {
            type: "array",
            title: title("endpoints"),
            items: {
              type: "object",
              required: ["name", "verbs"],
              properties: { name: { ...text("endpoint", DNS1123), maxLength: 63 }, verbs: verbs(ENDPOINT_VERBS) },
            },
          },
        },
      },
    },
  };
}

export const agentProfileUiSchema: UiSchema = {
  tools: { "ui:widget": "checkboxes" },
  access: {
    kinds: { items: { verbs: { "ui:widget": "checkboxes" } } },
    endpoints: { items: { verbs: { "ui:widget": "checkboxes" } } },
  },
};

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

const texts = (values: (string | undefined)[] | undefined): string[] =>
  (values ?? []).flatMap((value) => filled(value) ?? []);

const number = (value: number | undefined): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

/** The form as the manifest the API stores; what the form has no field for travels on from `stored`. */
export function toAgentProfileManifest(form: AgentProfileForm, stored?: unknown): unknown {
  const effort = filled(form.model?.reasoningEffort);
  const routing = (stored as { spec?: { model?: { routing?: unknown } } } | undefined)?.spec?.model?.routing;
  const hosts = texts(form.egress?.allowedHosts);
  const budget = number(form.egress?.maxBytesPerRun);
  const tools = texts(form.tools);
  const operations = texts(form.access?.operations);
  const kinds = (form.access?.kinds ?? []).flatMap((grant) => {
    const kind = filled(grant.kind);
    return kind ? [{ kind, verbs: texts(grant.verbs) }] : [];
  });
  const endpoints = (form.access?.endpoints ?? []).flatMap((grant) => {
    const name = filled(grant.name);
    return name ? [{ name, verbs: texts(grant.verbs) }] : [];
  });
  const access = {
    ...(operations.length > 0 ? { operations } : {}),
    ...(kinds.length > 0 ? { kinds } : {}),
    ...(endpoints.length > 0 ? { endpoints } : {}),
  };
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "AgentProfile",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: "org" },
    spec: {
      role: form.role,
      runtime: { image: filled(form.runtime?.image), digest: filled(form.runtime?.digest) },
      model: {
        provider: form.model?.provider,
        name: filled(form.model?.name),
        maxTokensPerRun: number(form.model?.maxTokensPerRun),
        ...(effort ? { reasoningEffort: effort } : {}),
        ...(routing ? { routing } : {}),
      },
      limits: {
        stepsPerRun: number(form.limits?.stepsPerRun),
        wallClock: filled(form.limits?.wallClock),
        concurrentRunsPerOrganization: number(form.limits?.concurrentRunsPerOrganization),
        requestsPerMinute: number(form.limits?.requestsPerMinute),
        maxResponseBytes: number(form.limits?.maxResponseBytes),
      },
      egress: { ...(hosts.length > 0 ? { allowedHosts: hosts } : {}), ...(budget !== undefined ? { maxBytesPerRun: budget } : {}) },
      ...(tools.length > 0 ? { tools } : {}),
      workspace: {
        cpu: filled(form.workspace?.cpu),
        memory: filled(form.workspace?.memory),
        ephemeralStorage: filled(form.workspace?.ephemeralStorage),
      },
      // Absent means read-only operations and nothing else (AG-70), which an emptied block asks for.
      ...(Object.keys(access).length > 0 ? { access } : {}),
    },
  };
}

/** The stored manifest back as the form. */
export function fromAgentProfileManifest(manifest: unknown): AgentProfileForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Omit<AgentProfileForm, "name"> };
  return { ...envelope.spec, name: envelope.metadata?.name ?? "" };
}
