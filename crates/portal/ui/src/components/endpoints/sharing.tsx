import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import type { JsonSchema } from "../forms/types";
import type { EditableForm } from "../EditResourceDialog";
import { DNS1123, SLUG_PATTERN } from "../../schemas/kinds";
import { Badge } from "../ui";
import type { BadgeTone } from "../ui";

/**
 * Who an Endpoint is shared with, the way the gateway reads it (Architecture/04 §5, EP-14,
 * EP-15): `project-list` admits the owner and the listed projects, `organization` every
 * project of the repository, `public` everyone.
 */
export interface Sharing {
  audience: string;
  allowedProjects: string[];
}

export const SPACE_LABEL = "joinedcontext.com/space";

export const AUDIENCE_TONE: Record<string, BadgeTone> = {
  "project-list": "neutral",
  organization: "info",
  public: "warning",
};

export function sharingOf(endpoint: Manifest): Sharing {
  const spec = endpoint.spec as { audience?: string; allowedProjects?: string[] };
  return {
    audience: spec.audience ?? "project-list",
    allowedProjects: Array.isArray(spec.allowedProjects) ? spec.allowedProjects : [],
  };
}

export function spaceOf(endpoint: Manifest): string | undefined {
  const space = refName((endpoint.spec as { contextSpaceRef?: unknown } | undefined)?.contextSpaceRef);
  return space !== "" ? space : endpoint.metadata.labels?.[SPACE_LABEL];
}

/** Whether `project` may call an endpoint `owner` publishes: the gateway's own rule. */
export function admits(endpoint: Manifest, owner: string, project: string): boolean {
  if (owner === project) {
    return true;
  }
  const { audience, allowedProjects } = sharingOf(endpoint);
  switch (audience) {
    case "organization":
    case "public":
      return true;
    case "project-list":
      return allowedProjects.includes(project);
    default:
      return false;
  }
}

/**
 * Whether the gateway admits a person with `groups` to this endpoint at all (EP-14, T-2631): its
 * own rule for a human — `public` and `organization` admit anyone signed in, `project-list` a
 * member of the owning project or of one it lists. A page reads only through an endpoint that
 * admits the person, instead of fetching and meeting a 403.
 */
export function admitsPerson(endpoint: Manifest, groups: readonly string[], project: string): boolean {
  const { audience, allowedProjects } = sharingOf(endpoint);
  if (audience === "public" || audience === "organization") {
    return true;
  }
  // A project lists its own endpoints; a manifest that omits its namespace is the page's project's.
  const owner = endpoint.metadata.namespace ?? project;
  return audience === "project-list" && [owner, ...allowedProjects].some((project) => project !== "" && groups.includes(project));
}

const SHOWN_PROJECTS = 1;

/** The audience as a chip, and for `project-list` the projects it names, so the owner sees who. */
export function SharedWithBadge({ endpoint }: { endpoint: Manifest }): JSX.Element {
  const { t } = useTranslation();
  const { audience, allowedProjects } = sharingOf(endpoint);
  const listed = audience === "project-list";
  return (
    <div className="flex min-w-0 items-center gap-1 whitespace-nowrap">
      <Badge tone={AUDIENCE_TONE[audience] ?? "neutral"}>{t(`endpoints.audience.${audience}`)}</Badge>
      {listed && allowedProjects.length > 0 ? (
        // One line in a row (T-4989): one project named, the rest counted; every name stays
        // in the list a screen reader reads, and the title shows them all.
        <ul
          aria-label={t("endpoints.sharedWith")}
          title={allowedProjects.join(", ")}
          className="flex min-w-0 items-center gap-1 text-caption text-fg-muted"
        >
          {allowedProjects.map((name, index) => (
            <li key={name} className={index < SHOWN_PROJECTS ? undefined : "sr-only"}>
              <Badge mono>{name}</Badge>
            </li>
          ))}
          {allowedProjects.length > SHOWN_PROJECTS ? (
            <li aria-hidden="true">{t("endpoints.field.more", { count: allowedProjects.length - SHOWN_PROJECTS })}</li>
          ) : null}
        </ul>
      ) : null}
    </div>
  );
}

/** A DNS-1123 label out of whatever parts name the thing: lowercase, dashes, 63 characters. */
export function dns1123(...parts: string[]): string {
  return parts
    .join("-")
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/-+/g, "-")
    .slice(0, 63)
    .replace(/^-+|-+$/g, "");
}

/** `SharedSpaceReference`, field for field with jc-core's `SharedSpaceReferenceSpec` (EP-15). */
export const sharedSpaceReferenceSchema: JsonSchema = {
  type: "object",
  required: ["alias"],
  additionalProperties: false,
  // Exactly one of the two names the source (EP-77).
  oneOf: [{ required: ["endpointRef"] }, { required: ["endpointSlug"] }],
  properties: {
    endpointSlug: { type: "string", pattern: SLUG_PATTERN },
    endpointRef: {
      type: "object",
      required: ["project", "name"],
      additionalProperties: false,
      properties: {
        project: { type: "string", pattern: DNS1123, maxLength: 63 },
        name: { type: "string", pattern: DNS1123, maxLength: 63 },
      },
    },
    alias: { type: "string", pattern: DNS1123, maxLength: 63 },
  },
};

/** How often a peer's schema surface is mirrored: jc-core's `Schedule.interval` (DM-49). */
const INTERVAL_PATTERN = "^[1-9][0-9]*[smhd]$";

/**
 * The part of a reference its project changes in place (T-2570, EP-15, DM-49): the alias the space
 * is read by here and how often the peer's schema is mirrored. The source stays as it is, because
 * another source is another reference; whatever else the stored manifest holds is kept.
 */
export function sharedReferenceForm(t: (key: string) => string): EditableForm {
  return {
    schema: {
      type: "object",
      required: ["alias"],
      additionalProperties: false,
      properties: {
        alias: { type: "string", pattern: DNS1123, maxLength: 63 },
        interval: { type: "string", pattern: INTERVAL_PATTERN },
      },
    },
    uiSchema: {
      alias: {
        "ui:title": t("endpoints.shared.edit.alias"),
        "ui:description": t("endpoints.shared.edit.aliasHelp"),
        "ui:placeholder": "helsinki-liikenne",
      },
      interval: {
        "ui:title": t("endpoints.shared.edit.interval"),
        "ui:description": t("endpoints.shared.edit.intervalHelp"),
        "ui:placeholder": "6h",
      },
    },
    fromManifest: (manifest) => {
      const spec = ((manifest as Manifest).spec ?? {}) as {
        alias?: string;
        schedule?: { interval?: string };
      };
      return {
        alias: spec.alias ?? "",
        ...(spec.schedule?.interval ? { interval: spec.schedule.interval } : {}),
      };
    },
    toManifest: (form, stored) => {
      const manifest = stored as Manifest;
      const { schedule, ...rest } = (manifest.spec ?? {}) as { schedule?: Record<string, unknown> };
      const interval = typeof form.interval === "string" ? form.interval.trim() : "";
      // A new interval replaces the schedule; none keeps whatever else it held, or drops it for
      // the 24-hour default.
      const { interval: _dropped, ...kept } = schedule ?? {};
      const nextSchedule = interval ? { interval } : Object.keys(kept).length ? kept : undefined;
      return {
        apiVersion: manifest.apiVersion,
        kind: manifest.kind,
        metadata: manifest.metadata,
        spec: { ...rest, alias: form.alias, ...(nextSchedule ? { schedule: nextSchedule } : {}) },
      };
    },
  };
}

/**
 * The manifest "Use in this project" proposes: one `SharedSpaceReference` in the consumer
 * project, named after the source project and endpoint, its alias after the source project
 * and space so it never shadows a space of the consumer's own (Architecture/04 §5). It names
 * the source by project and name, which the loader resolves to the slug of whatever
 * environment it lands in, so a copied pair of projects keeps working (EP-77).
 */
export function referenceManifest(project: string, source: string, endpoint: Manifest) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "SharedSpaceReference",
    metadata: {
      name: dns1123(source, endpoint.metadata.name),
      namespace: project,
    },
    spec: {
      endpointRef: { project: source, name: endpoint.metadata.name },
      alias: dns1123(source, spaceOf(endpoint) ?? endpoint.metadata.name),
    },
  };
}

/**
 * The reference of `project` that points at the Endpoint `name` of `source`, when one is
 * declared already: by `endpointRef`, or by the slug of an older reference (EP-77).
 */
export function referenceTo(
  references: Manifest[],
  slug: string,
  source: string,
  name: string,
): Manifest | undefined {
  return references.find((reference) => {
    const spec = reference.spec as {
      endpointSlug?: string;
      endpointRef?: { project?: string; name?: string };
    };
    if (spec.endpointRef) {
      return spec.endpointRef.project === source && spec.endpointRef.name === name;
    }
    return slug !== "" && spec.endpointSlug === slug;
  });
}
