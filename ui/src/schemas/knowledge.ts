// The forms of the knowledge assistant's two kinds (MF-51, MF-52, T-3057), mirroring jc-core's
// `KnowledgeSourceSpec` and `AssistantDeploymentSpec` field for field, with their bounds.
import type { JsonSchema } from "../components/forms/types";
import type { ResourceProposal } from "../api/manifest";
import { DNS1123, words } from "./kinds";

/** Five-field cron, the shape jc-core accepts for `schedule`. */
export const CRON = "^\\S+ \\S+ \\S+ \\S+ \\S+$";
/** An absolute https address with a host, as a start URL is. */
export const HTTPS_URL = "^https://[^\\s/?#]+(/\\S*)?$";
/** An origin a deployment may be placed on: https, a host, a port at most, no path. */
export const ORIGIN = "^https://[^\\s/?#:*]+(:[0-9]{1,5})?$";
export const LANGUAGE = "^[a-z]{2}$";
/** A path pattern of a site, starting with a slash. */
export const SITE_PATH = "^/\\S*$";
/** A colour as #rrggbb. */
export const HEX_COLOR = "^#[0-9a-fA-F]{6}$";
/** The channels that answer people nobody signed in: each names its origins, a rate limit and a budget (MF-52). */
export const ANONYMOUS_CHANNELS = ["public", "ckan", "iframe"] as const;

export const SOURCE_TYPES = ["website", "ckan", "upload"] as const;
/** A CKAN organization name, as jc-core checks it. */
export const CKAN_NAME = "^[a-z0-9_-]{2,100}$";
/** A model as `jc-agent-proxy` is asked for it: `vendor/name`, lowercase. */
export const MODEL_ID = "^[a-z0-9._:-]+/[a-z0-9._:/-]+$";
export const VISIBILITIES = ["internal", "public"] as const;
export const CHANNELS = ["public", "internal", "ckan", "iframe"] as const;
export const PDF_POLICIES = ["include", "exclude"] as const;
/** What one answer may use: the sources and the connectors, the sources alone, or the connectors alone (MF-52, T-4281). */
export const ANSWER_ROUTES = ["both", "rag", "data"] as const;
/** The EU data-theme codes, the one category list (MF-60, AG-124); labels in `knowledge.category`. */
export const CATEGORIES = ["AGRI", "ECON", "EDUC", "ENER", "ENVI", "GOVE", "HEAL", "INTR", "JUST", "REGI", "SOCI", "TECH", "TRAN"] as const;
/** How a selector reaches an Endpoint (MF-55); a channel nobody signs in to takes `public` alone (MF-58). */
export const ACCESS_LEVELS = ["public", "organization", "project", "grant"] as const;
/** An NGSI-LD entity type short name (MF-61). */
export const ENTITY_TYPE = "^[A-Z][A-Za-z0-9]{1,63}$";
/** How the chat picks its language (`widget.language`, MF-52, T-4343); `browser` is the default. */
export const WIDGET_LANGUAGES = ["browser", "ask", "fixed"] as const;
/** A source or an Endpoint of the own project by its name, or of a project in `scope.projects` as `{project}/{name}` (MF-52). */
export const PROJECT_REF = "^([a-z0-9]([-a-z0-9]*[a-z0-9])?/)?[a-z0-9]([-a-z0-9]*[a-z0-9])?$";
/** The read-only tools of an Endpoint's MCP surface the assistant calls (jc-assistant's allowlist, T-4140). */
export const ENDPOINT_TOOLS = [
  "list_types",
  "get_type",
  "list_attributes",
  "query_entities",
  "get_entity",
  "get_attribute_value",
  "query_temporal",
  "retrieve_temporal",
  "describe_schema",
  "describe_access",
] as const;

export function knowledgeSourceSchema(t: (key: string) => string, catalogues: string[] = []): JsonSchema {
  return {
    type: "object",
    required: ["name", "source", "visibility"],
    properties: {
      categories: { type: "array", title: t("knowledge.field.categories"), items: { type: "string", ...words(t, "knowledge.category", CATEGORIES) }, maxItems: 13, uniqueItems: true },
      name: { type: "string", title: t("knowledge.field.name"), pattern: DNS1123, maxLength: 63 },
      source: { type: "string", title: t("knowledge.field.source"), default: "website", ...words(t, "knowledge.type", SOURCE_TYPES) },
      startUrls: {
        type: "array",
        title: t("knowledge.field.startUrls"),
        items: { type: "string", pattern: HTTPS_URL, maxLength: 2048 },
        maxItems: 20,
        uniqueItems: true,
      },
      ckanInstanceRef: {
        type: "string",
        title: t("knowledge.field.ckanInstanceRef"),
        ...(catalogues.length > 0 ? { enum: catalogues } : { pattern: DNS1123 }),
      },
      organizations: {
        type: "array",
        title: t("knowledge.field.organizations"),
        items: { type: "string", pattern: CKAN_NAME },
        maxItems: 10,
        uniqueItems: true,
      },
      upload: {
        type: "object",
        title: t("knowledge.field.upload"),
        properties: {
          maxBytes: { type: "integer", title: t("knowledge.field.uploadMaxBytes"), minimum: 1, maximum: 50 * 1024 * 1024, default: 20 * 1024 * 1024 },
          maxFiles: { type: "integer", title: t("knowledge.field.uploadMaxFiles"), minimum: 1, maximum: 500, default: 100 },
        },
      },
      sitemap: { type: "boolean", title: t("knowledge.field.sitemap"), default: true },
      // WordPress REST bases whose items' links are read before the sitemap (MF-51, T-4154).
      wordpressTypes: {
        type: "array",
        title: t("knowledge.field.wordpressTypes"),
        items: { type: "string", pattern: "^[a-z0-9_-]{1,64}$", maxLength: 64 },
        maxItems: 40,
        uniqueItems: true,
      },
      include: {
        type: "array",
        title: t("knowledge.field.include"),
        items: { type: "string", pattern: SITE_PATH, maxLength: 256 },
        maxItems: 50,
      },
      exclude: {
        type: "array",
        title: t("knowledge.field.exclude"),
        items: { type: "string", pattern: SITE_PATH, maxLength: 256 },
        maxItems: 50,
      },
      maxDepth: { type: "integer", title: t("knowledge.field.maxDepth"), minimum: 1, maximum: 10, default: 3 },
      maxPages: { type: "integer", title: t("knowledge.field.maxPages"), minimum: 1, maximum: 50_000, default: 1_000 },
      pdf: {
        type: "object",
        title: t("knowledge.field.pdf"),
        properties: {
          policy: { type: "string", title: t("knowledge.field.pdfPolicy"), default: "include", ...words(t, "knowledge.pdfPolicy", PDF_POLICIES) },
          maxBytes: { type: "integer", title: t("knowledge.field.pdfMaxBytes"), minimum: 1, maximum: 200 * 1024 * 1024, default: 50 * 1024 * 1024 },
          maxPages: { type: "integer", title: t("knowledge.field.pdfMaxPages"), minimum: 1, maximum: 2_000, default: 500 },
        },
      },
      offDomainDocuments: { type: "boolean", title: t("knowledge.field.offDomainDocuments"), default: false },
      schedule: { type: "string", title: t("knowledge.field.schedule"), pattern: CRON, maxLength: 120 },
      languages: {
        type: "array",
        title: t("knowledge.field.languages"),
        items: { type: "string", pattern: LANGUAGE },
        uniqueItems: true,
      },
      visibility: { type: "string", title: t("knowledge.field.visibility"), default: "internal", ...words(t, "knowledge.visibility", VISIBILITIES) },
    },
  };
}

/** One text, or one per language of `languages`, as jc-core takes `theme.title` and `theme.greeting` (T-4365). */
function localised(t: (key: string) => string, title: string, minLength: number, maxLength: number): JsonSchema {
  return {
    title,
    anyOf: [
      { type: "string", title: t("knowledge.field.oneText"), minLength, maxLength },
      {
        type: "object",
        title: t("knowledge.field.perLanguage"),
        propertyNames: { pattern: LANGUAGE },
        additionalProperties: { type: "string", minLength: 1, maxLength },
        minProperties: 1,
      },
    ],
  };
}

export function assistantDeploymentSchema(
  t: (key: string) => string,
  sources: string[] = [],
  endpoints: string[] = [],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "publicId", "channel"],
    // A channel nobody signs in to is refused by jc-core without its origins, a rate limit and a
    // budget (MF-52); the form says so at those fields before anything is sent (T-3220).
    if: { properties: { channel: { anyOf: ANONYMOUS_CHANNELS.map((channel) => ({ const: channel })) } }, required: ["channel"] },
    then: {
      required: ["allowedOrigins", "rateLimit", "budget"],
      properties: {
        allowedOrigins: { minItems: 1 },
        rateLimit: { required: ["requestsPerMinute", "perClientPerMinute"] },
        budget: { required: ["tokensPerDay", "tokensPerConversation"] },
      },
    },
    // An answer from the sources alone needs a source, one from the connectors alone a connector (MF-52, T-4281).
    allOf: [
      {
        if: { properties: { answer: { properties: { routes: { const: "rag" } }, required: ["routes"] } }, required: ["answer"] },
        then: { required: ["sources"], properties: { sources: { minItems: 1 } } },
      },
      {
        if: { properties: { answer: { properties: { routes: { const: "data" } }, required: ["routes"] } }, required: ["answer"] },
        then: { anyOf: [{ required: ["connectors"], properties: { connectors: { minItems: 1 } } }, { required: ["select"] }] },
      },
      // On a channel nobody signs in to, a selector reaches public Endpoints alone (MF-58).
      {
        if: { properties: { channel: { anyOf: ANONYMOUS_CHANNELS.map((channel) => ({ const: channel })) } }, required: ["channel"] },
        then: { properties: { select: { properties: { accessLevel: { items: { const: "public" } } } } } },
      },
    ],
    properties: {
      name: { type: "string", title: t("knowledge.field.name"), pattern: DNS1123, maxLength: 63 },
      publicId: { type: "string", title: t("knowledge.field.publicId"), pattern: DNS1123, maxLength: 63 },
      channel: { type: "string", title: t("knowledge.field.channel"), default: "internal", ...words(t, "knowledge.channel", CHANNELS) },
      systemPrompt: { type: "string", title: t("knowledge.field.systemPrompt"), maxLength: 8_000 },
      sources: {
        type: "array",
        title: t("knowledge.field.sources"),
        items: sources.length > 0 ? { type: "string", enum: sources } : { type: "string", pattern: PROJECT_REF },
        uniqueItems: true,
      },
      connectors: {
        type: "array",
        title: t("knowledge.field.connectors"),
        items: {
          type: "object",
          required: ["endpoint", "tools"],
          properties: {
            endpoint: {
              type: "string",
              title: t("knowledge.field.connectorEndpoint"),
              ...(endpoints.length > 0 ? { enum: endpoints } : { pattern: PROJECT_REF }),
            },
            tools: {
              type: "array",
              title: t("knowledge.field.connectorTools"),
              items: { type: "string", enum: [...ENDPOINT_TOOLS] },
              minItems: 1,
              uniqueItems: true,
            },
            timeoutSeconds: { type: "integer", title: t("knowledge.field.connectorTimeout"), minimum: 1, maximum: 120, default: 20 },
          },
        },
      },
      // The Endpoints a selector picks beside the connectors (MF-58, ADR-N-055 §2.2). No
      // `minItems` on its lists: the form would pad them with an empty entry, and an empty list
      // leaves the manifest anyway.
      select: {
        type: "object",
        title: t("knowledge.field.select"),
        description: t("knowledge.field.selectHint"),
        properties: {
          projects: { type: "array", title: t("knowledge.field.selectProjects"), items: { type: "string", pattern: DNS1123 }, maxItems: 50, uniqueItems: true },
          themes: { type: "array", title: t("knowledge.field.selectThemes"), items: { type: "string", ...words(t, "knowledge.category", CATEGORIES) }, maxItems: 13, uniqueItems: true },
          types: { type: "array", title: t("knowledge.field.selectTypes"), items: { type: "string", pattern: ENTITY_TYPE }, maxItems: 50, uniqueItems: true },
          accessLevel: { type: "array", title: t("knowledge.field.selectAccess"), items: { type: "string", ...words(t, "knowledge.accessLevel", ACCESS_LEVELS) }, uniqueItems: true },
        },
      },
      exclude: {
        type: "array",
        title: t("knowledge.field.excludeEndpoints"),
        items: endpoints.length > 0 ? { type: "string", enum: endpoints } : { type: "string", pattern: PROJECT_REF },
        maxItems: 50,
        uniqueItems: true,
      },
      categories: {
        type: "array",
        title: t("knowledge.field.categories"),
        items: { type: "string", ...words(t, "knowledge.category", CATEGORIES) },
        maxItems: 13,
        uniqueItems: true,
      },
      answer: {
        type: "object",
        title: t("knowledge.field.answer"),
        properties: {
          routes: { type: "string", title: t("knowledge.field.answerRoutes"), default: "both", ...words(t, "knowledge.answerRoutes", ANSWER_ROUTES) },
        },
      },
      feedback: { type: "boolean", title: t("knowledge.field.feedback"), default: true },
      widget: {
        type: "object",
        title: t("knowledge.field.widget"),
        properties: {
          language: { type: "string", title: t("knowledge.field.widgetLanguage"), default: "browser", ...words(t, "knowledge.widgetLanguage", WIDGET_LANGUAGES) },
          languageToggle: { type: "boolean", title: t("knowledge.field.languageToggle"), default: true },
        },
      },
      allowedOrigins: {
        type: "array",
        title: t("knowledge.field.allowedOrigins"),
        items: { type: "string", pattern: ORIGIN },
        uniqueItems: true,
      },
      rateLimit: {
        type: "object",
        title: t("knowledge.field.rateLimit"),
        properties: {
          // Like the budget's token limits: the form fills the object with its other defaults, and
          // jc-core refuses a rate limit without these two (T-4869).
          requestsPerMinute: { type: "integer", title: t("knowledge.field.requestsPerMinute"), minimum: 1, maximum: 600, default: 60 },
          perClientPerMinute: { type: "integer", title: t("knowledge.field.perClientPerMinute"), minimum: 1, maximum: 120, default: 5 },
          perClientPerHour: { type: "integer", title: t("knowledge.field.perClientPerHour"), minimum: 1, maximum: 7_200, default: 60 },
          perClientPerDay: { type: "integer", title: t("knowledge.field.perClientPerDay"), minimum: 1, maximum: 100_000, default: 200 },
          perClientPerMonth: { type: "integer", title: t("knowledge.field.perClientPerMonth"), minimum: 1, maximum: 100_000, default: 300 },
          streamsPerClient: { type: "integer", title: t("knowledge.field.streamsPerClient"), minimum: 1, maximum: 10, default: 2 },
        },
      },
      budget: {
        type: "object",
        title: t("knowledge.field.budget"),
        properties: {
          // The defaults are Architecture/22's example: an object the form fills with defaults has to
          // carry both, or jc-core refuses the budget as incomplete (T-4824).
          tokensPerDay: { type: "integer", title: t("knowledge.field.tokensPerDay"), minimum: 1, maximum: 100_000_000, default: 2_000_000 },
          tokensPerConversation: { type: "integer", title: t("knowledge.field.tokensPerConversation"), minimum: 1, maximum: 100_000_000, default: 40_000 },
          costPerDayUsd: { type: "number", title: t("knowledge.field.costPerDayUsd"), minimum: 0.01, maximum: 1_000, default: 3 },
          outputTokensPerAnswer: { type: "integer", title: t("knowledge.field.outputTokensPerAnswer"), minimum: 100, maximum: 4_000, default: 700 },
          messagesPerConversation: { type: "integer", title: t("knowledge.field.messagesPerConversation"), minimum: 1, maximum: 200, default: 30 },
          questionChars: { type: "integer", title: t("knowledge.field.questionChars"), minimum: 100, maximum: 4_000, default: 1_000 },
        },
      },
      theme: {
        type: "object",
        title: t("knowledge.field.theme"),
        properties: {
          title: localised(t, t("knowledge.field.themeTitle"), 1, 60),
          primaryColor: { type: "string", title: t("knowledge.field.primaryColor"), pattern: HEX_COLOR },
          greeting: localised(t, t("knowledge.field.greeting"), 0, 500),
        },
      },
      languages: {
        type: "array",
        title: t("knowledge.field.languages"),
        items: { type: "string", pattern: LANGUAGE },
        uniqueItems: true,
      },
      sandbox: { type: "boolean", title: t("knowledge.field.sandbox"), default: false },
      enabled: { type: "boolean", title: t("knowledge.field.enabled"), default: true },
      model: {
        type: "object",
        title: t("knowledge.field.model"),
        properties: {
          router: { type: "string", title: t("knowledge.field.routerModel"), pattern: MODEL_ID, maxLength: 100 },
          answer: { type: "string", title: t("knowledge.field.answerModel"), pattern: MODEL_ID, maxLength: 100 },
        },
      },
      // Per language of `languages`, which jc-core checks (MF-52).
      suggestions: {
        type: "object",
        title: t("knowledge.field.suggestions"),
        propertyNames: { pattern: LANGUAGE },
        additionalProperties: {
          type: "array",
          items: { type: "string", minLength: 1, maxLength: 120 },
          maxItems: 6,
        },
      },
      disclaimer: {
        type: "object",
        title: t("knowledge.field.disclaimer"),
        propertyNames: { pattern: LANGUAGE },
        additionalProperties: { type: "string", minLength: 1, maxLength: 400 },
      },
      scope: {
        type: "object",
        title: t("knowledge.field.scope"),
        properties: {
          projects: {
            type: "array",
            title: t("knowledge.field.scopeProjects"),
            items: { type: "string", pattern: DNS1123 },
            maxItems: 10,
            uniqueItems: true,
          },
        },
      },
    },
  };
}

/** The deployment's sources are picked from every project the person reads (`knowledgeSources`, T-4282); its groups are its UiSchema's. */
export const assistantDeploymentUiSchema = { sources: { "ui:field": "knowledgeSources" } };

/** The projects a deployment's `{project}/{name}` sources and connectors reach, which `scope.projects` must name (MF-52). */
export function projectsNamed(spec: Record<string, unknown>): string[] {
  const refs = [
    ...(Array.isArray(spec.sources) ? spec.sources : []),
    ...(Array.isArray(spec.connectors) ? spec.connectors.map((c) => (c as { endpoint?: unknown } | null)?.endpoint) : []),
  ];
  return [...new Set(refs.filter((ref): ref is string => typeof ref === "string" && ref.includes("/")).map((ref) => ref.split("/")[0]))];
}

/** What a form holds: the name beside the spec's own fields. */
export type KindForm = Record<string, unknown> & { name?: string };

/** Drops what a form left empty, so the manifest carries only what was said (jc-core defaults the rest). */
function said(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.length > 0 ? value : undefined;
  }
  if (value && typeof value === "object") {
    const kept = Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .map(([key, inner]) => [key, said(inner)] as const)
        .filter(([, inner]) => inner !== undefined),
    );
    return Object.keys(kept).length > 0 ? kept : undefined;
  }
  if (typeof value === "string") {
    return value.trim() === "" ? undefined : value;
  }
  return value;
}

/**
 * A form as the manifest it proposes. The form shows every field of the spec, so its spec is the
 * whole spec: a field cleared on the form leaves the manifest. A stored manifest keeps its
 * metadata (labels, annotations), which the form does not show.
 */
export function toKindManifest(
  kind: "KnowledgeSource" | "AssistantDeployment",
  project: string,
  form: KindForm,
  stored?: unknown,
): ResourceProposal {
  const { name, ...spec } = form;
  if (kind === "AssistantDeployment") {
    // A source or connector of another project is read only while that project is in scope:
    // picking it puts the project there, so the form never proposes what jc-core refuses (T-4281).
    const scope = (spec.scope ?? {}) as { projects?: unknown };
    const listed = Array.isArray(scope.projects) ? scope.projects.filter((p): p is string => typeof p === "string") : [];
    const needed = projectsNamed(spec).filter((p) => p !== project && !listed.includes(p));
    if (needed.length > 0) spec.scope = { ...scope, projects: [...listed, ...needed] };
  }
  // The form fills the upload limits' defaults whatever the type, and jc-core refuses them on a
  // website or catalogue source: no website source could be proposed (T-4869).
  if (kind === "KnowledgeSource" && spec.source !== "upload") delete spec.upload;
  const kept = (stored as { metadata?: Record<string, unknown> } | null | undefined)?.metadata ?? {};
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind,
    metadata: { ...kept, name: String(name ?? ""), namespace: project },
    spec: (said(spec) as Record<string, unknown> | undefined) ?? {},
  } as ResourceProposal;
}

/** A manifest as the form shows it. */
export function fromKindManifest(manifest: unknown): KindForm {
  const read = manifest as { metadata?: { name?: unknown }; spec?: unknown } | null | undefined;
  const spec = read?.spec && typeof read.spec === "object" ? (read.spec as Record<string, unknown>) : {};
  return { ...spec, name: typeof read?.metadata?.name === "string" ? read.metadata.name : "" };
}
