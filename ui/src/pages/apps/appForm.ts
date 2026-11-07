import { storedMetadata } from "../../api/manifest";
import { appClass, type APP_SOURCES } from "../../schemas/kinds";

/** One data need as the form holds it: the geo and time narrowing are one field each here. */
export interface AppDataNeedForm {
  contextSpaceRef: string;
  types: string[];
  attrs?: string[];
  operations: string[];
  representations?: string[];
  q?: string;
  scopeQ?: string;
  within?: string;
  window?: string;
  /** The App roles this need is granted to; none is every caller (AP-96). */
  roles?: string[];
}

export interface AppForm {
  name: string;
  kind: string;
  visibility: string;
  embeddable?: boolean;
  source: {
    from: (typeof APP_SOURCES)[number];
    path?: string;
    url?: string;
    ref?: string;
    subdirectory?: string;
  };
  build: { tool: string; version: string }[];
  dataNeeds: AppDataNeedForm[];
  csp?: { connectSrc?: string[]; frameAncestors?: string[] };
  limits?: { requestsPerMinute?: number; maxFileRows?: number };
  /** Where a server pod may connect besides its endpoint (AP-134). */
  egress?: { cidr: string; ports: number[] }[];
}

/** A text worth writing, trimmed, or nothing. */
function text(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

/** A list without blank entries, or nothing: the manifest reads as what it declares. */
function list(values: string[] | undefined): string[] | undefined {
  const kept = (values ?? []).map((value) => value.trim()).filter((value) => value !== "");
  return kept.length > 0 ? kept : undefined;
}

/** An object without its absent members, or nothing when none is left. */
function members<T extends Record<string, unknown>>(value: T): T | undefined {
  const kept = Object.fromEntries(Object.entries(value).filter(([, member]) => member !== undefined));
  return Object.keys(kept).length > 0 ? (kept as T) : undefined;
}

/**
 * The form as the manifest the API stores (AP-01…AP-20).
 *
 * `stored` is the manifest an edit started from. Its metadata travels on, and so does its
 * `lifecycle`: the form has no field for it, because publishing is the catalogue's own action with
 * its own confirmation (AP-20), and an edit may not quietly draft or publish an app.
 */
export function toAppEnvelope(project: string, form: AppForm, stored?: unknown): unknown {
  const kept = (stored as { spec?: Record<string, unknown> } | undefined)?.spec ?? {};
  const lifecycle = kept.lifecycle;
  const egress = (form.egress ?? [])
    .map((destination) => ({ cidr: destination.cidr.trim(), ports: destination.ports }))
    .filter((destination) => destination.cidr !== "");
  const source =
    form.source.from === "git"
      ? {
          git: members({
            url: text(form.source.url),
            ref: text(form.source.ref),
            path: text(form.source.subdirectory),
          }),
        }
      : { path: text(form.source.path) };
  const csp = form.csp
    ? members({ connectSrc: list(form.csp.connectSrc), frameAncestors: list(form.csp.frameAncestors) })
    : undefined;
  // The service quotas (AP-165) have no field here: they travel on as stored, and the two the form
  // edits replace theirs.
  const limits = members({
    ...((kept.limits as Record<string, unknown> | undefined) ?? {}),
    requestsPerMinute: form.limits?.requestsPerMinute,
    maxFileRows: form.limits?.maxFileRows,
  });
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: project },
    spec: {
      kind: form.kind,
      visibility: form.visibility,
      ...(form.embeddable ? { embeddable: true } : {}),
      ...(typeof lifecycle === "string" ? { lifecycle } : {}),
      source,
      // A list of pins in the form, a map in the manifest (AP-11).
      build: Object.fromEntries(form.build.map((pin) => [pin.tool.trim(), pin.version.trim()])),
      dataNeeds: form.dataNeeds.map((need) =>
        members({
          contextSpaceRef: { kind: "ContextSpace", name: need.contextSpaceRef },
          types: list(need.types) ?? [],
          attrs: list(need.attrs),
          operations: list(need.operations) ?? [],
          representations: list(need.representations),
          q: text(need.q),
          scopeQ: text(need.scopeQ),
          geoQ: text(need.within) ? { within: { scopeRef: text(need.within) } } : undefined,
          temporalQ: text(need.window) ? { window: text(need.window) } : undefined,
          roles: list(need.roles),
        }),
      ),
      ...(csp ? { csp } : {}),
      ...(limits ? { limits } : {}),
      ...(egress.length > 0 ? { egress } : {}),
      // The App's roles and who holds them have no field here; an edit keeps them as stored
      // rather than dropping them (AP-90, AP-91).
      ...(kept.roles !== undefined ? { roles: kept.roles } : {}),
      // The App's services are the Services section's to switch (AP-163); an edit keeps them.
      ...(kept.services !== undefined ? { services: kept.services } : {}),
      ...(kept.access !== undefined ? { access: kept.access } : {}),
    },
  };
}

/** The stored manifest back as the form: the same pair, so the YAML view and the fields agree. */
export function fromAppEnvelope(manifest: unknown): AppForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const source = (spec.source ?? {}) as {
    path?: string;
    git?: { url?: string; ref?: string; path?: string };
  };
  const needs = (spec.dataNeeds as Record<string, unknown>[] | undefined) ?? [];
  return {
    name: envelope.metadata?.name ?? "",
    kind: appClass(spec.kind as string | undefined),
    visibility: (spec.visibility as string | undefined) ?? "project",
    embeddable: spec.embeddable === true,
    source: source.git
      ? {
          from: "git",
          url: source.git.url ?? "",
          ref: source.git.ref ?? "",
          ...(source.git.path ? { subdirectory: source.git.path } : {}),
        }
      : { from: "path", path: source.path ?? "" },
    build: Object.entries((spec.build as Record<string, string> | undefined) ?? {}).map(
      ([tool, version]) => ({ tool, version }),
    ),
    dataNeeds: needs.map((need) => {
      const reference = need.contextSpaceRef as string | { name?: string } | undefined;
      const within = (need.geoQ as { within?: { scopeRef?: string } } | undefined)?.within?.scopeRef;
      const window = (need.temporalQ as { window?: string } | undefined)?.window;
      return {
        contextSpaceRef: (typeof reference === "string" ? reference : reference?.name) ?? "",
        types: (need.types as string[] | undefined) ?? [],
        operations: (need.operations as string[] | undefined) ?? [],
        ...(need.attrs ? { attrs: need.attrs as string[] } : {}),
        ...(need.representations ? { representations: need.representations as string[] } : {}),
        ...(need.q ? { q: need.q as string } : {}),
        ...(need.scopeQ ? { scopeQ: need.scopeQ as string } : {}),
        ...(within ? { within } : {}),
        ...(window ? { window } : {}),
        ...(Array.isArray(need.roles) && need.roles.length > 0 ? { roles: need.roles as string[] } : {}),
      };
    }),
    ...(spec.csp ? { csp: spec.csp as AppForm["csp"] } : {}),
    ...(spec.limits ? { limits: spec.limits as AppForm["limits"] } : {}),
    ...(Array.isArray(spec.egress) && spec.egress.length > 0 ? { egress: spec.egress as AppForm["egress"] } : {}),
  };
}
