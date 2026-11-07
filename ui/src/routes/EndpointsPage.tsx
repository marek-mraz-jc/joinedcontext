import { FormHeading, useFormRoute } from "../components/forms/FormRoute";
import { CopyUrlButton } from "../components/endpoints/links";
import { ConnectAssistant } from "../components/endpoints/ConnectAssistant";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { importManifests, proposeChecked } from "../api/proposal";
import { asManifests, isChange, localized, overlay, plainTitle, refName } from "../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../api/manifest";
import type { Verdict } from "../api/drafts";
import { useProjectOrganization, useProjects } from "../api/projects";
import { takePrefill } from "../assistant/state";
import { useProjectUsage } from "../components/ProjectQuota";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ChangeNotice } from "../components/ChangeNotice";
import { ManifestGrid } from "../components/grid/ManifestGrid";
import { ManifestYamlDialog } from "../components/grid/ManifestYamlDialog";
import { endpointRows, SharedRowActions } from "../pages/endpoints/endpointRows";
import type { EndpointRow } from "../pages/endpoints/endpointRows";
import { CALLERS, projectUsageQuery, SURFACES } from "../api/endpointUsage";
import type { ProjectUsage } from "../api/endpointUsage";
import { DeleteResourceAction } from "../components/DeleteResourceDialog";
import { EditResourceDialog } from "../components/EditResourceDialog";
import type { ResourceTarget } from "../components/DeleteResourceDialog";
import { RowActions } from "../components/ui/RowActions";
import type { RowAction } from "../components/ui/RowActions";
import { usePermissions } from "../api/permissions";
import { SaveAsResourceAction } from "../components/SaveAsDialog";
import { WorkOnCopyAction } from "../components/WorkOnCopyDialog";
import { ExportButton } from "../components/export/ExportButton";
import { SchemaProjectionPanel } from "../pages/endpoints/SchemaProjectionPanel";
import {
  endpointUrl,
  alwaysServed,
  RepresentationSummary,
  servedRepresentations,
} from "../components/endpoints/links";
import { useBranding } from "../branding";
import {
  admits,
  referenceManifest,
  SharedWithBadge,
  SPACE_LABEL,
  spaceOf,
} from "../components/endpoints/sharing";
import {
  ALWAYS_ON_REPRESENTATIONS,
  endpointSchema,
  endpointUiSchema,
  generateSlug,
  REPRESENTATIONS,
} from "../schemas/kinds";
import { disabledRepresentations, surfaceFitQuery, surfaceGaps } from "../api/surfaceFit";
import type { JsonSchema } from "../components/forms/types";
import { ModelPicker } from "../pages/endpoints/ModelPicker";
import { FilterProof, classesWithSlots } from "../pages/endpoints/FilterProof";
import type { FilterDraft } from "../pages/endpoints/FilterProof";
import { spaceSegment } from "../components/endpoints/policyBinding";
import { catalogForm, catalogOf, prefillFromOrganization } from "../pages/endpoints/catalog";
import type { CatalogForm, CatalogManifest } from "../pages/endpoints/catalog";
import type { ModelPickerState } from "../pages/endpoints/ModelPicker";
import { EndpointDataView } from "../components/entities/EndpointDataView";
import { CodeBlock,
  buttonClass,
  Alert,
  Badge,
  Button,
  Dialog,
  EmptyState,
  Icon,
  Input,
  PageHeader,
  safeHref,
} from "../components/ui";

interface EndpointForm {
  name: string;
  title?: string;
  contextSpaceRef: string;
  slug?: string;
  audience: string;
  enabledRepresentations: string[];
  allowedProjects?: string[];
  rateLimits?: { requestsPerMinute?: number; burst?: number };
  caching?: { maxAgeSeconds?: number };
  /** The open-data publication; the catalogue's name is what turns it on (EP-62). */
  publish?: {
    ckan?: {
      instanceRef?: string;
      organization?: string;
      name?: string;
    };
  };
  /** The catalogue description, one string per text in the author's language (EP-78, UI-50). */
  catalog?: CatalogForm;
  /** The named outputs per format (EP-125, API/11 EP-159, EP-163), kept as written. */
  representations?: Record<string, unknown>;
}

/**
 * The outputs the manifest keeps: calendars only with the iCalendar box, which jc-core requires;
 * feeds and the widget's map defaults always, so editing an Endpoint never drops them (T-5239).
 */
export function keptRepresentations(representations: Record<string, unknown> | undefined, checked: string[]): { representations?: Record<string, unknown> } {
  const { ical, ...rest } = representations ?? {};
  const kept = { ...rest, ...(ical && checked.includes("ical") ? { ical } : {}) };
  return Object.keys(kept).length > 0 ? { representations: kept } : {};
}

/** The `publish` block of the manifest, or nothing when no catalogue is named (EP-62). */
function publishOf(form: EndpointForm): { publish?: Record<string, unknown> } {
  const ckan = form.publish?.ckan;
  const catalogue = ckan?.instanceRef?.trim();
  if (!ckan || !catalogue) {
    return {};
  }
  const organization = ckan.organization?.trim();
  const dataset = ckan.name?.trim();
  return {
    publish: {
      ckan: {
        instanceRef: { kind: "CkanInstance", name: catalogue },
        ...(organization ? { organization } : {}),
        ...(dataset ? { name: dataset } : {}),
      },
    },
  };
}

/** The stored `publish` block as the form holds it: the reference is its name. */
function publishForm(spec: {
  publish?: {
    ckan?: {
      instanceRef?: string | { name?: string };
      organization?: string;
      name?: string;
    };
  };
}): EndpointForm["publish"] {
  const ckan = spec.publish?.ckan;
  if (!ckan) {
    return undefined;
  }
  const reference = ckan.instanceRef;
  return {
    ckan: {
      instanceRef: (typeof reference === "string" ? reference : reference?.name) ?? "",
      ...(ckan.organization ? { organization: ckan.organization } : {}),
      ...(ckan.name ? { name: ckan.name } : {}),
    },
  };
}

/**
 * The form's checked standards for a spec: what it serves (EP-104), in the form's order, the
 * always-on ones included so they show checked.
 */
export function checkedRepresentations(spec: {
  enabledRepresentations?: unknown;
  disabledRepresentations?: unknown;
  mcp?: unknown;
}): string[] {
  const served = servedRepresentations(spec);
  return REPRESENTATIONS.filter((rep) => served.includes(rep));
}

/**
 * What the checked standards write (EP-104). An Endpoint that lists its set keeps a list, in
 * the order it had, so saving never widens it and a reviewer reads one changed line; so does
 * one that serves `sta`, which only a list turns on. Every other Endpoint lists nothing and
 * names the unchecked ones in `disabledRepresentations`, so a standard added later reaches it.
 * The always-on ones are never written as disabled: jc-core refuses that (EP-10, EP-24).
 */
export function representationSpec(
  checked: string[],
  stored: { enabledRepresentations?: unknown; mcp?: unknown } = {},
): { enabledRepresentations?: string[]; disabledRepresentations?: string[] } {
  const always = alwaysServed(stored);
  const chosen = checked.filter((rep) => !always.includes(rep));
  const listed = Array.isArray(stored.enabledRepresentations)
    ? stored.enabledRepresentations.filter((rep): rep is string => typeof rep === "string")
    : undefined;
  // SensorThings and iCalendar are served only where a list names them (EP-104, EP-125).
  if (listed || chosen.includes("sta") || chosen.includes("ical")) {
    const kept = (listed ?? []).filter((rep) => always.includes(rep) || chosen.includes(rep));
    const list = [...kept, ...chosen.filter((rep) => !kept.includes(rep))];
    return { enabledRepresentations: list.length > 0 ? list : ["ngsi-ld"] };
  }
  const disabled = REPRESENTATIONS.filter(
    (rep) => rep !== "sta" && rep !== "ical" && rep !== "mcp" && !always.includes(rep) && !chosen.includes(rep),
  );
  return disabled.length > 0 ? { disabledRepresentations: disabled } : {};
}

function toSpec(
  form: EndpointForm,
  slug: string,
  hiddenAttributes: string[],
  projectionRefName?: string,
  storedCatalog?: CatalogManifest,
  language = "en",
  stored: { enabledRepresentations?: unknown; mcp?: unknown } = {},
) {
  const {
    allowedProjects,
    rateLimits,
    caching,
    publish,
    catalog: catalogValues,
    enabledRepresentations: checked,
    representations,
    ...rest
  } = form;
  void publish;
  const catalog = catalogOf(catalogValues, storedCatalog, language);
  delete (rest as Partial<EndpointForm>).name;
  delete (rest as Partial<EndpointForm>).title;
  return {
    ...rest,
    ...representationSpec(checked ?? [], stored),
    ...keptRepresentations(representations, checked ?? []),
    slug,
    ...publishOf(form),
    ...(catalog ? { catalog } : {}),
    ...(form.audience === "project-list" && allowedProjects && allowedProjects.length > 0
      ? { allowedProjects }
      : {}),
    ...(typeof rateLimits?.requestsPerMinute === "number"
      ? {
          rateLimits: {
            requestsPerMinute: rateLimits.requestsPerMinute,
            ...(typeof rateLimits.burst === "number" ? { burst: rateLimits.burst } : {}),
          },
        }
      : {}),
    ...(typeof caching?.maxAgeSeconds === "number"
      ? { caching: { maxAgeSeconds: caching.maxAgeSeconds } }
      : {}),
    ...(hiddenAttributes.length > 0 ? { projection: { hiddenAttributes } } : {}),
    ...(projectionRefName
      ? { projectionRef: { kind: "ModelProjection", name: projectionRefName } }
      : {}),
  };
}

/** The spec keys the form writes; the rest of an edited manifest travels as it is (T-0885). */
const OWNED_SPEC = [
  "contextSpaceRef",
  "slug",
  "audience",
  "enabledRepresentations",
  "disabledRepresentations",
  "allowedProjects",
  "rateLimits",
  "caching",
  "projection",
  "projectionRef",
  "publish",
  "catalog",
];

export function toEnvelope(
  project: string,
  form: EndpointForm,
  slug: string,
  hiddenAttributes: string[],
  projectionRefName?: string,
  base?: Manifest,
  language = "en",
): Manifest {
  const { name, title } = form;
  const baseSpec = (base?.spec ?? {}) as {
    catalog?: CatalogManifest;
    enabledRepresentations?: unknown;
    mcp?: unknown;
  };
  const spec = toSpec(
    form,
    slug,
    hiddenAttributes,
    projectionRefName,
    baseSpec.catalog,
    language,
    baseSpec,
  );
  const next: Manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name,
      namespace: project,
      labels: { [SPACE_LABEL]: form.contextSpaceRef },
      ...(title?.trim() ? { title } : {}),
    },
    spec,
  };
  return overlay(base, next, OWNED_SPEC);
}

export function toForm(endpoint: Manifest): EndpointForm {
  const spec = endpoint.spec as {
    contextSpaceRef?: string | { name?: string };
    slug?: string;
    audience?: string;
    enabledRepresentations?: string[];
    disabledRepresentations?: string[];
    mcp?: boolean;
    allowedProjects?: string[];
    rateLimits?: { requestsPerMinute?: number; burst?: number };
    caching?: { maxAgeSeconds?: number };
    publish?: {
      ckan?: {
        instanceRef?: string | { name?: string };
        organization?: string;
        name?: string;
      };
    };
    catalog?: CatalogManifest;
    representations?: Record<string, unknown>;
  };
  const catalog = catalogForm(spec.catalog);
  return {
    name: endpoint.metadata.name,
    title: plainTitle(endpoint.metadata.title),
    contextSpaceRef: refName(spec.contextSpaceRef),
    slug: spec.slug ?? "",
    audience: spec.audience ?? "project-list",
    enabledRepresentations: checkedRepresentations(spec),
    allowedProjects: spec.allowedProjects ?? [],
    rateLimits: spec.rateLimits,
    caching: spec.caching,
    publish: publishForm(spec),
    ...(catalog ? { catalog } : {}),
    ...(spec.representations ? { representations: spec.representations } : {}),
  };
}

function hiddenOf(endpoint: Manifest): string[] {
  const projection = (endpoint.spec as { projection?: { hiddenAttributes?: string[] } })
    .projection;
  return projection?.hiddenAttributes ?? [];
}

/** Whether the gateway serves this endpoint yet: only a Live one answers a read (T-1433). */
function isLive(endpoint: Manifest | null): boolean {
  return (endpoint?.status?.phase ?? "").toLowerCase() === "live";
}

/** `schema` without `allowedProjects`: the manifest refuses the list outside `project-list`. */
function withoutBurst(schema: JsonSchema): JsonSchema {
  const rateLimits = schema.properties?.rateLimits as JsonSchema | undefined;
  if (!rateLimits?.properties) {
    return schema;
  }
  const { burst, ...kept } = rateLimits.properties;
  void burst;
  return { ...schema, properties: { ...schema.properties, rateLimits: { ...rateLimits, properties: kept } } };
}

function withoutAllowedProjects(schema: JsonSchema): JsonSchema {
  const properties = Object.fromEntries(
    Object.entries(schema.properties ?? {}).filter(([key]) => key !== "allowedProjects"),
  );
  return { ...schema, properties };
}

/**
 * The actions of one endpoint row (T-2287, UI-26, UI-44).
 *
 * The cell used to paint eight controls side by side — explore, copy, export, edit, save as, work on a
 * copy, delete and the source — which is what the owner counted. What it answers stays in the open,
 * because that is why a person opens this table; the rest is behind the one `⋯` menu, and every dialog
 * is rendered beside the menu, never inside it (Radix unmounts the menu's content when it closes).
 */
function EndpointRowActions({
  project,
  endpoint,
  label,
  onEdit,
}: {
  project: string;
  endpoint: Manifest;
  label: string;
  onEdit: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  const navigate = useNavigate();
  const [openAction, setOpenAction] = useState<"yaml" | "export" | "saveAs" | "copy" | "delete" | null>(null);
  const name = endpoint.metadata.name;
  const slug = (endpoint.spec as { slug?: string }).slug ?? "";
  const target: ResourceTarget = { project, kind: "Endpoint", plural: "endpoints", name, label };
  const denied = (verb: "propose" | "delete") =>
    permissions.can("Endpoint", verb) ? undefined : t("permissions.denied", { verb, kind: "Endpoint" });

  // One menu per row (T-6095): the row itself opens the endpoint, its data is on that page, and the
  // Git file is an item here instead of an icon column.
  const sourceUrl = endpoint.status?.sourceUrl ? safeHref(endpoint.status.sourceUrl) : undefined;
  const actions: RowAction[] = [
    {
      key: "open",
      label: t("resourceList.open"),
      onSelect: () => void navigate({ to: "/projects/$project/$plural/$name", params: { project, plural: "endpoints", name } }),
    },
    { key: "edit", label: t("endpoints.edit"), onSelect: onEdit, disabledReason: denied("propose") },
    { key: "yaml", label: t("grid.yaml.action"), onSelect: () => setOpenAction("yaml") },
    ...(sourceUrl ? [{ key: "source", label: t("resourceList.viewSource"), href: sourceUrl }] : []),
    ...(slug
      ? [
          {
            key: "copyUrl",
            label: t("endpoints.copyUrl"),
            onSelect: () => {
              void navigator.clipboard?.writeText(endpointUrl(slug, "")).catch(() => undefined);
            },
          },
        ]
      : []),
    { key: "export", label: t("export.action"), onSelect: () => setOpenAction("export") },
    {
      key: "saveAs",
      label: t("saveAs.button"),
      onSelect: () => setOpenAction("saveAs"),
      disabledReason: denied("propose"),
    },
    { key: "copy", label: t("workspaces.open.action"), onSelect: () => setOpenAction("copy") },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setOpenAction("delete"),
      disabledReason: denied("delete"),
    },
  ];

  // A row's dialogs mount while open only: a page of 500 endpoints otherwise renders 2 000 closed
  // dialogs with their hooks (T-3538).
  return (
    <>
      <RowActions label={label} actions={actions} />
      {openAction === "yaml" ? <ManifestYamlDialog manifest={endpoint} label={label} onClose={() => setOpenAction(null)} /> : null}
      {openAction === "export" ? (
        <ExportButton
          project={project}
          target={{ plural: "endpoints", name }}
          label={t("export.action")}
          trigger={false}
          open
          onOpenChange={(next) => setOpenAction(next ? "export" : null)}
        />
      ) : null}
      {openAction === "saveAs" ? (
        <SaveAsResourceAction
          target={target}
          trigger={false}
          open
          onOpenChange={(next) => setOpenAction(next ? "saveAs" : null)}
        />
      ) : null}
      {openAction === "copy" ? (
        <WorkOnCopyAction
          project={project}
          scope={{ kind: "resources", items: [{ kind: "Endpoint", name }] }}
          trigger={false}
          open
          onOpenChange={(next) => setOpenAction(next ? "copy" : null)}
        />
      ) : null}
      {openAction === "delete" ? (
        <DeleteResourceAction
          target={target}
          trigger={false}
          open
          onOpenChange={(next) => setOpenAction(next ? "delete" : null)}
        />
      ) : null}
    </>
  );
}

type Calls = ProjectUsage["endpoints"][number];

/** The calls by surface and by kind of caller, each a label and its number, most calls first, the empty ones left out. */
function useCallSplit(calls: Calls | undefined) {
  const { t, i18n } = useTranslation();
  const numbers = new Intl.NumberFormat(i18n.resolvedLanguage ?? i18n.language ?? "en");
  const split = (counts: Record<string, number> | undefined, names: readonly string[], key: string) =>
    names
      .filter((name) => (counts?.[name] ?? 0) > 0)
      .map((name) => ({ label: t(`${key}.${name}`), count: counts?.[name] ?? 0 }))
      .sort((a, b) => b.count - a.count)
      .map((part) => ({ label: part.label, count: numbers.format(part.count) }));
  const groups = [
    { label: t("endpoints.calls.bySurface"), parts: split(calls?.totals, SURFACES, "endpointUsage.surface") },
    { label: t("endpoints.calls.byCaller"), parts: split(calls?.callers, CALLERS, "endpoints.calls.caller") },
  ].filter((group) => group.parts.length > 0);
  return { total: numbers.format(calls?.total ?? 0), groups };
}

/** Why a row has no count: another project publishes it, or the usage is loading or failed. */
export type NotCounted = "shared" | "loading" | "failed";

/**
 * An Endpoint's calls in the last 7 days and, beside the total, the surfaces they came through
 * (T-6169) and the kinds of caller that made them: people, workloads, agents, nobody signed in
 * (T-6186). The gateways count kinds, never who called (T-5118). Exported for its test.
 */
export function CallsCell({ calls, notCounted }: { calls: Calls | undefined; notCounted?: NotCounted }): JSX.Element {
  const { t } = useTranslation();
  const { total, groups } = useCallSplit(calls);
  if (notCounted) {
    const reason = t(`endpoints.calls.notCounted.${notCounted}`);
    return (
      <span className="text-fg-subtle" title={reason}>
        —<span className="sr-only"> {reason}</span>
      </span>
    );
  }
  const said = groups.map((group) => `${group.label}: ${group.parts.map((part) => `${part.label} ${part.count}`).join(" · ")}`);
  // One line per row (T-4989): the split is the cell's tooltip, a line per grouping, read out with
  // the number; where a pointer can hover, a dotted underline says there is one. The row's Details
  // panel shows it whole, on a phone too.
  return (
    <span
      className={`whitespace-nowrap tabular-nums${said.length > 0 ? " [@media(hover:hover)]:cursor-help [@media(hover:hover)]:underline [@media(hover:hover)]:decoration-dotted [@media(hover:hover)]:underline-offset-4" : " text-fg-muted"}`}
      title={said.join("\n") || t("endpoints.calls.none")}
    >
      {total}
      <span className="sr-only"> ({said.length > 0 ? said.join("; ").replaceAll(" · ", ", ") : t("endpoints.calls.none")})</span>
    </span>
  );
}

/**
 * The same calls in the cell panel, read-only and shown rather than hovered (T-6186): the total,
 * the split, or in words why there is none. Exported for its test.
 */
export function CallsDetail({ calls, notCounted }: { calls: Calls | undefined; notCounted?: NotCounted }): JSX.Element {
  const { t } = useTranslation();
  const { total, groups } = useCallSplit(calls);
  return (
    <div className="flex flex-col gap-3 rounded-md bg-surface-subtle px-3 py-2" data-read-only>
      {notCounted ? (
        <p className="text-fg-muted">{t(`endpoints.calls.notCounted.${notCounted}`)}</p>
      ) : (
        <>
          <span className="font-semibold tabular-nums">{total}</span>
          {groups.length === 0 ? <p className="text-caption text-fg-muted">{t("endpoints.calls.none")}</p> : null}
          {groups.map((group) => (
            <div key={group.label} className="flex flex-col gap-1">
              <span className="text-caption font-semibold text-fg-muted">{group.label}</span>
              <ul className="grid w-full grid-cols-[minmax(0,1fr)_auto] gap-x-8 gap-y-1" aria-label={group.label}>
                {group.parts.map((part) => (
                  <li key={part.label} className="contents">
                    <span className="text-fg-muted">{part.label}</span>{" "}
                    <span className="text-right font-semibold tabular-nums">{part.count}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </>
      )}
    </div>
  );
}

/** Whether the endpoint's name or title holds `needle`, case aside. */
function matches(endpoint: Manifest, needle: string, locale: string): boolean {
  const wanted = needle.trim().toLowerCase();
  return (
    endpoint.metadata.name.toLowerCase().includes(wanted) ||
    localized(endpoint.metadata.title, locale, "").toLowerCase().includes(wanted)
  );
}

/** A ticked writable class of the projection picker, with its own narrowing. */
interface WritableClass {
  name: string;
  idPattern?: string;
  scope?: string;
  writeQ?: string;
}

/**
 * The `{endpoint}-write` Policy of a projected Endpoint. Each class keeps its own scope and write
 * filter: classes that share them are one rule, as before; classes that differ get one rule each
 * in `rules[]` (ADR-N-057), so no class is narrowed by another's (F-UI3-003).
 */
export function writePolicyOf(
  project: string,
  form: { name: string; contextSpaceRef: string },
  classes: WritableClass[],
): Manifest {
  const ruleOf = (group: WritableClass[]) => ({
    assignee: { kind: "role", id: `${project}-writers` },
    operations: ["updateOps"],
    information: group.map((c) => ({ entities: [{ type: c.name, ...(c.idPattern ? { idPattern: c.idPattern } : {}) }] })),
    ...(group[0]?.scope ? { scopeQ: group[0].scope } : {}),
    ...(group[0]?.writeQ ? { q: group[0].writeQ } : {}),
  });
  const shared = classes.every((c) => (c.scope ?? "") === (classes[0]?.scope ?? "") && (c.writeQ ?? "") === (classes[0]?.writeQ ?? ""));
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: {
      name: `${form.name}-write`,
      namespace: project,
      labels: { [SPACE_LABEL]: form.contextSpaceRef },
    },
    spec: {
      contextSpaceRef: { kind: "ContextSpace", name: form.contextSpaceRef },
      assigner: "did:web:{orgDomain}",
      ...(shared ? ruleOf(classes) : { rules: classes.map((c) => ruleOf([c])) }),
    },
  } as Manifest;
}

export function EndpointsPage({ project, edit }: { project: string; edit?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const { domain } = useBranding();
  const queryClient = useQueryClient();
  const permissions = usePermissions(project);
  const usage = useProjectUsage(project);
  // Calls in the last 7 days per own Endpoint, as the gateways count them (T-5118, T-6169).
  const usage7d = useQuery(projectUsageQuery(project, 7));
  const viewOnly = !permissions.isLoading && !permissions.can("Endpoint", "propose");
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  // The language a catalogue text the form shows is written back in (UI-50).
  const language = locale.slice(0, 2);

  const [urlDraftName, setUrlDraftName] = useState(() => {
    if (typeof window === "undefined") return undefined;
    return new URLSearchParams(window.location.search).get("draft") ?? undefined;
  });

  const [activeSlug, setActiveSlug] = useState<string>(() => generateSlug());
  // The rows on screen and the name or title they are narrowed by (T-4545): 10 000 rows mounted at
  // once took 18 s to show the first; a page of them shows at once and the rest wait behind a button.
  const [needle, setNeedle] = useState("");

  const [pickerState, setPickerState] = useState<ModelPickerState>({
    projectionName: "",
    classes: {},
  });

  // What the assistant hands the page: the form values, the attributes to hide, and `existing`
  // when it opened an endpoint that is already there rather than drafting a new one.
  const [prefill] = useState(
    () =>
      takePrefill(window.location.pathname) as
        | (Partial<EndpointForm> & { hiddenAttributes?: string[]; entityTypes?: string[]; existing?: boolean })
        | null,
  );
  const [editing, setEditing] = useState<EndpointForm | null>(() => {
    if (!urlDraftName && !prefill) {
      return null;
    }
    const form: Partial<EndpointForm> & { hiddenAttributes?: string[]; entityTypes?: string[]; existing?: boolean } = {
      ...prefill,
    };
    delete form.hiddenAttributes;
    delete form.entityTypes;
    delete form.existing;
    // A form offers only what an Endpoint may serve: a retired file a prefill still names is not
    // checked, so it never stops the form's validation (T-4600).
    if (Array.isArray(form.enabledRepresentations)) {
      form.enabledRepresentations = form.enabledRepresentations.filter((rep) =>
        (REPRESENTATIONS as readonly string[]).includes(rep),
      );
    }
    return {
      name: "",
      contextSpaceRef: "",
      audience: "project-list",
      enabledRepresentations: checkedRepresentations({}),
      allowedProjects: [],
      ...form,
      ...(urlDraftName ? { name: urlDraftName } : {}),
    };
  });
  const [isNew, setIsNew] = useState(
    prefill?.existing !== true && (prefill !== null || urlDraftName !== undefined),
  );
  // The classes the assistant's proposal exposes; none named means every class of the model.
  const [handedClasses] = useState(() =>
    prefill && prefill.existing !== true
      ? (prefill.entityTypes ?? []).filter((c): c is string => typeof c === "string")
      : undefined,
  );
  // The manifest the form edits, from the list or from the YAML view: what it does not show
  // travels with the proposal unchanged (T-0885).
  const [base, setBase] = useState<Manifest | null>(null);
  // The endpoint whose slug, hidden attributes and projection the page took over, once.
  const [adopted, setAdopted] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [hidden, setHidden] = useState<string[]>(() =>
    Array.isArray(prefill?.hiddenAttributes)
      ? prefill.hiddenAttributes.filter((a): a is string => typeof a === "string")
      : [],
  );
  const [connectOpen, setConnectOpen] = useState(false);

  const list = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  // An endpoint the assistant opened by name edits that endpoint: a PUT to its name with its own
  // slug, never a new endpoint with a fresh one. Only a draft or a prefill that says so is taken
  // over; a name typed into a new endpoint's form is left for the API to refuse.
  const openedName =
    urlDraftName !== undefined && (prefill === null || prefill.existing === true)
      ? urlDraftName
      : prefill?.existing === true && typeof prefill.name === "string"
        ? prefill.name
        : undefined;
  const opened = openedName
    ? asManifests(list.data?.items ?? []).find((endpoint) => endpoint.metadata.name === openedName)
    : undefined;
  if (opened && adopted !== opened.metadata.name) {
    setAdopted(opened.metadata.name);
    setIsNew(false);
    setBase(opened);
    const own = toForm(opened);
    const ownSlug = own.slug || (typeof prefill?.slug === "string" ? prefill.slug : "");
    if (ownSlug) {
      setActiveSlug(ownSlug);
    }
    if (!Array.isArray(prefill?.hiddenAttributes)) {
      setHidden(hiddenOf(opened));
    }
    const projectionRef = (opened.spec as { projectionRef?: { name?: string } })?.projectionRef?.name;
    setPickerState({
      projectionName: projectionRef || opened.metadata.name,
      selectedProjectionRef: projectionRef,
      classes: {},
    });
    // A draft the dialog loads replaces these; without one the form starts from the endpoint
    // as it is, with whatever the assistant changed on top.
    if (editing && !editing.contextSpaceRef) {
      const changed: Partial<EndpointForm> & { hiddenAttributes?: string[]; entityTypes?: string[]; existing?: boolean } = {
        ...(prefill?.existing === true ? prefill : {}),
      };
      delete changed.hiddenAttributes;
      delete changed.entityTypes;
      delete changed.existing;
      setEditing({ ...own, ...changed, name: own.name, slug: own.slug });
    }
  }

  // The projections, for the filter the proof reads beside the form (T-2776); the picker reads the
  // same list under the same key, so this asks nothing twice.
  const projectionsQuery = useQuery({
    queryKey: queryKeys.list(project, "projections"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "projections" } },
        }),
      ),
  });

  const spacesQuery = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });

  const formRoute = useFormRoute();
  const routedNew = formRoute?.form?.mode === "new";
  // What only the form reads waits for the form: the list shows none of it (F-R2UI4-012). A form
  // address (`/endpoints/new`, `?edit=`) counts as open, so its prefill and pickers load at once.
  const formOpen = editing !== null || Boolean(formRoute?.form) || edit !== undefined;
  // The Organization, whose name, domain and open-data desk a new endpoint's catalogue starts
  // from (EP-78, EP-80), and the pipelines a catalogue may name as the data's origin.
  // The project's own Organization: with several, the first of the list is another body's.
  const organization = useProjectOrganization(project);
  const catalogPrefill = useMemo(
    () => prefillFromOrganization(organization.organization),
    [organization.organization],
  );
  const pipelinesQuery = useQuery({
    queryKey: queryKeys.list(project, "pipelines"),
    enabled: formOpen,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "pipelines" } },
        }),
      ),
  });

  // The catalogues this project may publish to (EP-62). A project with none leaves the open-data
  // field free text, which is what a manifest written before the catalogue carries anyway.
  const cataloguesQuery = useQuery({
    queryKey: queryKeys.list(project, "ckaninstances"),
    enabled: formOpen,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "ckaninstances" } },
        }),
      ),
  });

  const modelsQuery = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    enabled: formOpen,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  // The other projects of the repository: what the allowed-project picker offers, and whose
  // endpoints the "shared with this project" section reads (PF-05, EP-14).
  const projects = useProjects();
  const others = (projects.data ?? []).filter((name) => name !== project);
  const otherLists = useQueries({
    queries: others.map((other) => ({
      queryKey: queryKeys.list(other, "endpoints"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project: other, plural: "endpoints" } },
          }),
        ),
    })),
  });
  const referencesQuery = useQuery({
    queryKey: queryKeys.list(project, "shared"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "shared" } },
        }),
      ),
  });

  const [verdict, setVerdict] = useState<Verdict | null>(null);

  const editedSpace = editing?.contextSpaceRef;
  // What the space's model cannot serve greys its checkbox out, with the reason under the form
  // (EP-125, T-5299); propose refuses the same surfaces with the same sentences.
  const fitQuery = useQuery(surfaceFitQuery(project, editedSpace ?? ""));
  const fitGaps = editedSpace ? surfaceGaps(fitQuery.data) : [];
  const spaceHasModel =
    editedSpace !== undefined &&
    asManifests(modelsQuery.data?.items ?? []).some(
      (m) => (m.spec as { contextSpaceRef?: string })?.contextSpaceRef === editedSpace,
    );

  const tickedClassNames = Object.keys(pickerState.classes).filter(
    (c) => pickerState.classes[c]?.ticked,
  );
  // In a space with a model a new endpoint publishes the ticked classes as its own projection
  // and policies, and so does an existing one whose classes are ticked; otherwise the endpoint is
  // proposed alone, with the projection it already names.
  const projecting =
    spaceHasModel && !pickerState.selectedProjectionRef && (isNew || tickedClassNames.length > 0);
  const projectionNameOf = (form: EndpointForm) =>
    pickerState.projectionName || form.name || "projection";

  /** The Endpoint the form stands for: what its draft holds, what is checked, what is proposed. */
  const endpointOf = (form: EndpointForm) =>
    toEnvelope(
      project,
      form,
      activeSlug,
      hidden,
      projecting ? projectionNameOf(form) : pickerState.selectedProjectionRef,
      base ?? undefined,
      language,
    );

  /**
   * The filter this form stands for, as the proof previews it: the ticked classes and their slots
   * when it projects, else the projection the endpoint names; the conditions of that projection,
   * which the endpoint's own page edits; and the hidden attributes of this form.
   */
  const formDraft = (form: EndpointForm): FilterDraft => {
    const named = projecting ? projectionNameOf(form) : pickerState.selectedProjectionRef;
    const saved = asManifests(projectionsQuery.data?.items ?? []).find((p) => p.metadata.name === named);
    return {
      classes: projecting
        ? tickedClassNames.map((name) => ({ name, slots: pickerState.classes[name].slots }))
        : saved
          ? classesWithSlots(saved)
          : [],
      filter: (saved?.spec as { filter?: Record<string, string> } | undefined)?.filter ?? {},
      hiddenAttributes: hidden,
    };
  };

  const buildManifests = (form: EndpointForm) => {
    const endpointEnvelope = endpointOf(form);
    if (!projecting) {
      return [endpointEnvelope];
    }
    const projectionName = projectionNameOf(form);

    const modelProjection = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ModelProjection",
      metadata: {
        name: projectionName,
        namespace: project,
        labels: { [SPACE_LABEL]: form.contextSpaceRef },
      },
      spec: {
        contextSpaceRef: form.contextSpaceRef,
        dataModelRef: {
          kind: "DataModel",
          name: pickerState.dataModelName ?? "",
          version: pickerState.dataModelVersion ?? "1",
        },
        classes: tickedClassNames.map((cName) => ({
          name: cName,
          slots: pickerState.classes[cName].slots,
        })),
      },
    };

    const readInfo = tickedClassNames.map((cName) => {
      const slots = pickerState.classes[cName].slots;
      return {
        entities: [{ type: cName }],
        ...(slots.length > 0 ? { propertyNames: slots } : {}),
      };
    });

    const readPolicy = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: {
        name: `${form.name}-read`,
        namespace: project,
        labels: { [SPACE_LABEL]: form.contextSpaceRef },
      },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: form.contextSpaceRef },
        assigner: "did:web:{orgDomain}",
        assignee:
          form.audience === "public"
            ? { kind: "role", id: "public" }
            : { kind: "role", id: `${project}-readers` },
        operations: ["retrieveOps"],
        information: readInfo,
      },
    };

    const writableClassNames = tickedClassNames.filter((c) => pickerState.classes[c]?.writable);
    const manifests = [modelProjection, endpointEnvelope, readPolicy];

    if (writableClassNames.length > 0) {
      manifests.push(
        writePolicyOf(
          project,
          form,
          writableClassNames.map((name) => ({ name, ...pickerState.classes[name] })),
        ),
      );
    }

    return manifests;
  };

  /** The projection, its policies and the endpoint as one import; `dryRun` checks without proposing. */
  const importBundle = async (form: EndpointForm, dryRun: boolean): Promise<unknown> => {
    if (tickedClassNames.length === 0) {
      throw new Error(t("endpoints.picker.nothingTicked"));
    }
    // The bundle is the form's own endpoint, its projection and its policies, not a foreign
    // import, so a projection an earlier proposal already drew is updated by this one instead
    // of refusing the whole check (MF-23, T-1227).
    return importManifests(project, buildManifests(form), dryRun);
  };

  // A bundle is checked whole, and the endpoint is always checked with its draft too, so the
  // verdict the proposal waits for is recorded for exactly the manifest the form shows (AG-62).
  const check = useMutation({
    mutationFn: async (form: EndpointForm) => {
      setFormError(null);
      if (projecting) {
        await importBundle(form, true);
      }
      const envelope = endpointOf(form);
      const draftRef = form.name ? { kind: "Endpoint", name: form.name } : undefined;
      const body = (draftRef ? { ...envelope, draft: draftRef } : envelope);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" }, query: { dryRun: "All" } },
          body,
        }),
      );
    },
    onSuccess: (result) => {
      const answer = result as { verdict?: Verdict };
      if (answer?.verdict) {
        setVerdict(answer.verdict);
      }
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  // The assistant's change opens checked: the form rebuilds the manifest from its fields, which
  // leaves the verdict the assistant recorded behind, so the page checks what it shows once.
  const checkedOnOpen = useRef(false);
  const openedOnChange = prefill?.existing === true && adopted !== null && modelsQuery.isSuccess;
  useEffect(() => {
    if (checkedOnOpen.current || !openedOnChange || !editing?.contextSpaceRef) {
      return;
    }
    checkedOnOpen.current = true;
    check.mutate(editing);
  }, [openedOnChange, editing, check]);

  const propose = useMutation({
    mutationFn: async ({
      form,
      create,
      draft: draftRef,
    }: {
      form: EndpointForm;
      create: boolean;
      draft?: { kind: string; name: string };
    }) => {
      setFormError(null);
      if (projecting) {
        return importBundle(form, false);
      }
      const envelope = endpointOf(form);
      const body = (draftRef ? { ...envelope, draft: draftRef } : envelope);
      const result = create
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "endpoints" } },
            body,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural: "endpoints", name: form.name } },
            body,
          });
      return unwrap(result);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setEditing(null);
      setUrlDraftName(undefined);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "endpoints") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "projections") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  // One click declares the SharedSpaceReference; it lands as a Change like every write (EP-15).
  const reference = useMutation({
    mutationFn: async ({ source, endpoint }: { source: string; endpoint: Manifest }) =>
      proposeChecked(
        project,
        "shared",
        referenceManifest(project, source, endpoint) as ResourceProposal,
        true,
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "shared") });
    },
  });

  const endpoints = asManifests(list.data?.items ?? []);

  /** Open this page's own editor on one endpoint: the row's action and `?edit=<name>` (T-2281). */
  const openEditor = useCallback((endpoint: Manifest) => {
    setFormError(null);
    setIsNew(false);
    setBase(endpoint);
    setUrlDraftName(endpoint.metadata.name);
    setHidden(hiddenOf(endpoint));
    const parsedForm = toForm(endpoint);
    setActiveSlug(parsedForm.slug || generateSlug());
    const projectionRefName = (endpoint.spec as { projectionRef?: { name?: string } })?.projectionRef
      ?.name;
    setPickerState({
      projectionName: projectionRefName || endpoint.metadata.name,
      selectedProjectionRef: projectionRefName,
      classes: {},
    });
    setEditing(parsedForm);
  }, []);

  /** A blank endpoint in the editor: the add button, or the address `/endpoints/new` (T-2474). */
  // A new endpoint's catalogue starts from the Organization: its name, its domain and its
  // open-data desk, never a person (EP-80).
  const startNew = useCallback((firstSpace: string, catalog?: CatalogForm) => {
    setFormError(null);
    setIsNew(true);
    setBase(null);
    setUrlDraftName(undefined);
    setHidden([]);
    setActiveSlug(generateSlug());
    setPickerState({ projectionName: "", classes: {} });
    setEditing({
      name: "",
      contextSpaceRef: firstSpace,
      audience: "project-list",
      enabledRepresentations: checkedRepresentations({}),
      allowedProjects: [],
      ...(catalog ? { catalog } : {}),
    });
  }, []);

  // `/endpoints/new`: the blank form once, after the spaces have answered, so the first space is
  // preselected as the button does it.
  const openedNew = useRef(false);
  useEffect(() => {
    if (!routedNew) {
      openedNew.current = false;
      return;
    }
    if (openedNew.current || spacesQuery.isPending || !organization.settled) {
      return;
    }
    openedNew.current = true;
    startNew(asManifests(spacesQuery.data?.items ?? [])[0]?.metadata.name ?? "", catalogPrefill);
  }, [routedNew, spacesQuery.isPending, organization.settled, spacesQuery.data, startNew, catalogPrefill]);

  // `?edit=<name>` from the endpoint's own settings page: the editor opens once, on the endpoint the
  // URL names, and only when the list has it — a name nobody publishes opens nothing rather than an
  // empty form that would propose a new endpoint (T-2281).
  // Once per name the address carries: the list stays mounted while its forms come and go
  // (T-2474), so the next edit address opens its own endpoint.
  const openedFromUrl = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!edit) {
      openedFromUrl.current = undefined;
      return;
    }
    if (openedFromUrl.current === edit) {
      return;
    }
    const wanted = endpoints.find((endpoint) => endpoint.metadata.name === edit);
    if (!wanted) {
      return;
    }
    openedFromUrl.current = edit;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    openEditor(wanted);
  }, [edit, endpoints, openEditor]);
  const spaceNames = asManifests(spacesQuery.data?.items ?? []).map((s) => s.metadata.name);
  const references = asManifests(referencesQuery.data?.items ?? []);
  const shared = others.flatMap((source, index) =>
    asManifests(otherLists[index]?.data?.items ?? [])
      .filter((endpoint) => admits(endpoint, source, project))
      .map((endpoint) => ({ source, endpoint })),
  );
  // One grid of own, shared and gone-reference rows (T-6169): a reference whose Endpoint no longer
  // shows stays a row, because a row nothing lists is a grant nobody can remove (T-0706, EP-15).
  const rows = endpointRows(project, endpoints, shared, references);
  const matchingRows = needle.trim()
    ? rows.filter((row) =>
        row.endpoint ? matches(row.endpoint, needle, locale) : (row.reference?.metadata.name ?? "").includes(needle.trim().toLowerCase()),
      )
    : rows;
  const sharedPending =
    projects.isPending || referencesQuery.isPending || otherLists.some((other) => other.isPending);
  const notCounted = (own: boolean): NotCounted | undefined =>
    !own ? "shared" : usage7d.isError ? "failed" : usage7d.isSuccess ? undefined : "loading";
  const callsByName = new Map((usage7d.data?.endpoints ?? []).map((each) => [each.name, each]));
  // Once per render: `text` runs per row, and a t() per row costs 10 000 Endpoints their first paint (T-5181).
  const ownSource = t("endpoints.source.own");
  const goneSource = t("endpoints.shared.goneSource");
  const rowLabel = (row: EndpointRow) =>
    row.endpoint
      ? localized(row.endpoint.metadata.title, locale, row.endpoint.metadata.name)
      : String((row.reference?.spec as { alias?: string } | undefined)?.alias ?? "") || (row.reference?.metadata.name ?? "");
  const pickable =
    others.length > 0
      ? [
          ...others,
          ...(editing?.allowedProjects ?? []).filter(
            (name) => typeof name === "string" && name.length > 0 && !others.includes(name),
          ),
        ]
      : [];
  const catalogueNames = asManifests(cataloguesQuery.data?.items ?? []).map(
    (instance) => instance.metadata.name,
  );
  // A catalogue the endpoint already names and this list does not: the manifest keeps it, so the
  // form opens every endpoint the API accepted rather than dropping its publication (T-2400).
  // A pipeline the catalogue already names stays a choice, as a catalogue the endpoint names does.
  const namedPipeline = editing?.catalog?.pipelineRef;
  const pipelineNames = [
    ...new Set([
      ...asManifests(pipelinesQuery.data?.items ?? []).map((pipeline) => pipeline.metadata.name),
      ...(namedPipeline ? [namedPipeline] : []),
    ]),
  ];
  const named = editing?.publish?.ckan?.instanceRef;
  const catalogues =
    named && named.length > 0 && !catalogueNames.includes(named)
      ? [...catalogueNames, named]
      : catalogueNames;
  const baseSchema = endpointSchema(
    t,
    spaceNames,
    pickable,
    editing?.rateLimits?.requestsPerMinute,
    catalogues,
    pipelineNames,
  );
  const audienceSchema =
    editing?.audience === "project-list" ? baseSchema : withoutAllowedProjects(baseSchema);
  // A burst is part of a limit: without a rate per minute there is nothing for it to exceed,
  // and a burst alone is dropped on the way out, so the field shows once a limit is picked.
  const schema =
    typeof editing?.rateLimits?.requestsPerMinute === "number"
      ? audienceSchema
      : withoutBurst(audienceSchema);
  const uiSchema = {
    ...endpointUiSchema,
    enabledRepresentations: {
      ...endpointUiSchema.enabledRepresentations,
      "ui:enumDisabled": disabledRepresentations(
        ALWAYS_ON_REPRESENTATIONS,
        editedSpace ? fitQuery.data : undefined,
        editing?.enabledRepresentations,
      ),
    },
    rateLimits: { requestsPerMinute: { "ui:options": { emptyLabel: t("endpoints.rateClass.none") } } },
    ...(pickable.length > 0 ? { allowedProjects: { "ui:widget": "checkboxes" } } : {}),
  };

  // Rebuilt on every render: a handful of small objects, and no hook after the early returns.
  const previewManifests = editing ? buildManifests(editing) : null;

  // The same control in the header and in the empty list (T-1381). On a routed list it goes to
  // `/endpoints/new`, and the address opens the form (T-2474).
  const addButton = (
    <PermissionGuard project={project} kind="Endpoint" verb="propose">
      <Button
        variant="primary"
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => (formRoute ? formRoute.openNew() : startNew(spaceNames[0] ?? "", catalogPrefill))}
      >
        {t("endpoints.add")}
      </Button>
    </PermissionGuard>
  );

  // The reason before the person types: a project at its public-endpoint quota refuses the next
  // public one, and until T-1594 it said so only after the form was filled and submitted. The
  // control stays enabled, because an endpoint for the project or the organization does not count
  // against this dimension (PF-73, PF-75).
  const publicEndpoints = usage.data?.publicEndpoints;
  const publicFull =
    publicEndpoints?.limit !== undefined && (publicEndpoints.used ?? 0) >= publicEndpoints.limit;

  // The public address: on top of an existing endpoint's form, where it is copied from; under the
  // fields of a new one, so the form opens on its first field at 400 px (T-6163).
  const address = (
    <div className="flex flex-col gap-1 rounded border border-border bg-surface-subtle px-3 py-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-caption font-medium text-fg">{t("endpoints.field.publicUrl")}</span>
        <code data-testid="endpoint-url" className="min-w-0 break-all font-mono text-caption text-fg-muted">
          {/* The copied address with the slug picked out: one builder for both (F-R2UI4-015). */}
          {endpointUrl("", "")}
          <span data-testid="endpoint-slug" className="font-semibold text-fg">
            {activeSlug}
          </span>
        </code>
        <CopyUrlButton url={endpointUrl(activeSlug, "")} />
      </div>
      <p className="text-xs text-fg-subtle">{t("endpoints.slugHint")}</p>
    </div>
  );

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        compact
        title={t("endpoints.title")}
        description={t("endpoints.lead")}
        search={
          endpoints.length > 0 ? (
            <Input
              type="search"
              aria-label={t("endpoints.find")}
              placeholder={t("endpoints.find")}
              value={needle}
              onChange={(event) => {
                setNeedle(event.target.value);
              }}
            />
          ) : undefined
        }
        actions={
          <>
            {/* Nothing to connect to while the project publishes nothing (as before T-6095). */}
            {endpoints.length > 0 ? (
              <Button variant="secondary" icon={<Icon name="chat" className="size-4" />} onClick={() => setConnectOpen(true)}>
                {t("endpoints.hub.button")}
              </Button>
            ) : null}
            {addButton}
          </>
        }
      />

      {publicFull ? (
        <Alert role="status" tone="warning">
          {t("quota.dimensionExceeded", {
            limit: publicEndpoints?.limit,
            dimension: t("quota.dimension.publicEndpoints"),
          })}
        </Alert>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <ManifestGrid<EndpointRow>
        caption={t("endpoints.title")}
        storageKey="endpoints"
        searchLabel={t("endpoints.find")}
        search={needle}
        onSearchChange={setNeedle}
        rows={matchingRows}
        total={rows.length}
        rowKey={(row) => row.key}
        // Pending until the shared lists answered too, or a project that publishes nothing itself
        // flashes "no endpoints" before its shared rows arrive.
        query={{ ...list, isPending: list.isPending || sharedPending, refetch: list.refetch }}
        name={{
          id: "name",
          label: t("endpoints.field.name"),
          text: rowLabel,
          // The name opens the endpoint's own page, where every setting and every surface link is
          // (T-2281); the whole row opens it, so the name is no underlined link (T-4989).
          cell: (row) => {
            const endpoint = row.endpoint;
            if (!endpoint) {
              return (
                <div className="flex min-w-0 max-w-xs flex-wrap items-baseline gap-2">
                  <span className="truncate font-mono">{rowLabel(row)}</span>
                  <Badge tone="warning">{t("endpoints.shared.gone")}</Badge>
                </div>
              );
            }
            if (!row.own) {
              return (
                <div className="flex min-w-0 max-w-60 items-baseline gap-2 whitespace-nowrap">
                  <Link
                    data-row-link=""
                    to="/projects/$project/$plural/$name"
                    params={{ plural: "endpoints", project: row.source ?? project, name: endpoint.metadata.name }}
                    className="focus-ring max-w-full shrink-0 truncate rounded-sm text-fg hover:text-primary-soft-fg"
                  >
                    {rowLabel(row)}
                  </Link>
                  {endpoint.metadata.title ? (
                    <span className="hidden truncate font-mono text-caption font-normal text-fg-subtle 2xl:inline">
                      {endpoint.metadata.name}
                    </span>
                  ) : null}
                  {row.reference ? <Badge tone="success">{t("endpoints.shared.referenced")}</Badge> : null}
                </div>
              );
            }
            return (
            <div className="flex min-w-0 max-w-60 items-baseline gap-2 whitespace-nowrap">
              <Link
                data-row-link=""
                to="/projects/$project/$plural/$name"
                params={{ plural: "endpoints", project, name: endpoint.metadata.name }}
                className="focus-ring max-w-full shrink-0 truncate rounded-sm text-fg hover:text-primary-soft-fg"
              >
                {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
              </Link>
              {/* Below `sm` a row is the title, the state and the action (UI-27, T-4214). */}
              {endpoint.metadata.title ? (
                <span className="hidden truncate font-mono text-caption font-normal text-fg-subtle 2xl:inline">
                  {endpoint.metadata.name}
                </span>
              ) : null}
              {/* An App's own endpoint, written by the Portal beside it (T-2759): said so, since
                  nobody created it on this page and editing it here is undone by the App. */}
              {endpoint.metadata.annotations?.["joinedcontext.com/generated-by"] === "portal/app-reconciler" ? (
                <span className="hidden truncate text-caption font-normal text-fg-subtle sm:inline">
                  {t("endpoints.ofApp", { app: endpoint.metadata.name.replace(/^app-/, "") })}
                </span>
              ) : null}
            </div>
          );
          },
        }}
        columns={[
          {
            id: "source",
            label: t("endpoints.source.label"),
            filter: true,
            text: (row) => (row.own ? ownSource : (row.source ?? goneSource)),
            cell: (row) =>
              row.own ? (
                <span className="text-fg-muted">{t("endpoints.source.own")}</span>
              ) : row.source ? (
                <Badge tone="info" className="whitespace-nowrap" data-source={row.source}>{t("endpoints.source.shared", { project: row.source })}</Badge>
              ) : (
                <span className="text-fg-muted">{t("endpoints.shared.goneSource")}</span>
              ),
          },
          {
            id: "space",
            label: t("grid.field.space"),
            field: "contextSpaceRef",
            filter: true,
            // Under Columns, beside the filter chips (T-6169): eight columns do not fit 1440 px.
            hidden: true,
            text: (row) => (row.endpoint ? (spaceOf(row.endpoint) ?? "") : ""),
            // One line per row (T-4989): a long space name is cut, whole in its tooltip.
            cell: (row) => {
              const space = (row.endpoint && spaceOf(row.endpoint)) || "";
              return (
                <span className="block max-w-48 truncate font-mono text-caption" title={space || undefined}>
                  {space || "—"}
                </span>
              );
            },
          },
          {
            id: "audience",
            label: t("endpoints.field.audience"),
            field: "audience",
            filter: true,
            text: (row) => String((row.endpoint?.spec as { audience?: string } | undefined)?.audience ?? ""),
            valueLabel: (value) => t(`endpoints.audience.${value}`, { defaultValue: value }),
            cell: (row) => (row.endpoint ? <SharedWithBadge endpoint={row.endpoint} /> : "—"),
          },
          {
            id: "representations",
            label: t("endpoints.field.representations"),
            field: "enabledRepresentations",
            valueLabel: (value) => t(`spaces.api.surface.${value}`, { defaultValue: value }),
            text: (row) => (row.endpoint ? servedRepresentations(row.endpoint.spec as { enabledRepresentations?: string[] }).join(", ") : ""),
            cell: (row) =>
              row.endpoint ? <RepresentationSummary served={servedRepresentations(row.endpoint.spec as { enabledRepresentations?: string[] })} /> : "—",
          },
          { id: "mcp", label: t("grid.field.mcp"), field: "mcp", hidden: true, text: (row) => ((row.endpoint?.spec as { mcp?: boolean | null } | undefined)?.mcp === false ? t("grid.off") : t("grid.on")) },
          {
            id: "calls",
            label: t("endpoints.calls.label"),
            numeric: true,
            align: "right",
            // Counted per publishing project: a shared Endpoint's calls are its owner's to read.
            text: (row) => (row.own && row.endpoint ? String(callsByName.get(row.endpoint.metadata.name)?.total ?? 0) : ""),
            cell: (row) => <CallsCell calls={row.own && row.endpoint ? callsByName.get(row.endpoint.metadata.name) : undefined} notCounted={notCounted(row.own)} />,
            detail: (row) => <CallsDetail calls={row.own && row.endpoint ? callsByName.get(row.endpoint.metadata.name) : undefined} notCounted={notCounted(row.own)} />,
          },
          {
            id: "phase",
            label: t("endpoints.field.phase"),
            filter: true,
            text: (row) => (row.own ? (row.endpoint?.status?.phase ?? "") : ""),
            cell: (row) => (row.own ? <LifecycleBadge kind="phase" value={row.endpoint?.status?.phase} /> : "—"),
          },
        ]}
        actions={(row) =>
          row.own && row.endpoint ? (
            <EndpointRowActions
              project={project}
              endpoint={row.endpoint}
              label={rowLabel(row)}
              onEdit={() => {
                const endpoint = row.endpoint as Manifest;
                return formRoute ? formRoute.openEdit(endpoint.metadata.name) : openEditor(endpoint);
              }}
            />
          ) : (
            <SharedRowActions
              project={project}
              row={row}
              label={row.source ? `${row.source}/${rowLabel(row)}` : rowLabel(row)}
              useBusy={reference.isPending}
              onUse={() => (row.endpoint && row.source ? reference.mutate({ source: row.source, endpoint: row.endpoint }) : undefined)}
            />
          )
        }
        edit={{ project, plural: "endpoints", kind: "Endpoint", manifest: (row) => (row.own ? row.endpoint : null), onChange: setChange }}
        empty={
          rows.length > 0 ? (
            // A search that matches nothing names the text and offers the way back (T-6095).
            <EmptyState
              bare
              icon="search"
              title={t("endpoints.searchNoMatch", { text: needle.trim() })}
              description={t("endpoints.searchNoMatchHint")}
              action={
                <Button variant="secondary" size="sm" onClick={() => setNeedle("")}>
                  {t("spaces.inside.clearSearch")}
                </Button>
              }
            />
          ) : (
          <EmptyState
            bare
            icon="endpoints"
            title={t("endpoints.empty")}
            description={t("endpoints.addHint")}
            action={
              <div className="flex flex-wrap items-center justify-center gap-3">
                {addButton}
                {/* What other projects publish is where a person finds one to use here (T-3246). */}
                <Link to="/endpoints" className={buttonClass("secondary", "md")}>
                  {t("endpoints.shared.browse")}
                </Link>
              </div>
            }
          />
          )
        }
      />
      {reference.isError ? (
        <Alert role="alert" tone="danger">
          {reference.error instanceof ApiError
            ? (reference.error.problem?.detail ?? reference.error.message)
            : t("app.error.generic")}
        </Alert>
      ) : null}

      {editing !== null && !isNew && base && viewOnly ? (
        // A person who may not propose an endpoint still opens one: filled and closed, not the
        // editor whose proposal the server would refuse (T-2875).
        <EditResourceDialog
          readOnly
          open
          target={{ project, kind: "Endpoint", plural: "endpoints", name: base.metadata.name }}
          onOpenChange={(open) => {
            if (!open) {
              setEditing(null);
              setUrlDraftName(undefined);
            }
          }}
        />
      ) : (
      <ResourceFormDialog<EndpointForm>
        kind="Endpoint"
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setEditing(null);
            setUrlDraftName(undefined);
          }
        }}
        project={project}
        draftKind="Endpoint"
        draftName={editing?.name || urlDraftName || undefined}
        verdict={verdict}
        onVerdictChange={setVerdict}
        onCheck={(form) => {
          if (form.name) {
            check.mutate(form);
          }
        }}
        source={{
          toManifest: endpointOf,
          fromManifest: (manifest) => {
            setBase(manifest as Manifest);
            return toForm(manifest as Manifest);
          },
        }}
        title={isNew ? t("endpoints.add") : t("endpoints.edit")}
        description={t("endpoints.addHint")}
        schema={schema}
        uiSchema={uiSchema}
        formData={editing ?? undefined}
        submitLabel={t("endpoints.propose")}
        submitting={propose.isPending}
        error={formError}
        onSubmit={(form, draftRef) => propose.mutate({ form, create: isNew, draft: draftRef })}
        onChange={(form) => {
          if (form) {
            setEditing(form);
          }
        }}
        afterFields={
          <>
            {isNew ? address : null}
            {/* UI-58, T-0958: a form the assistant filled says so, so the person reads the
                values as a proposal to check rather than as something they typed. */}
            {prefill ? (
              <Alert role="note" tone="info">
                {t("assistant.prefilledNotice")}
              </Alert>
            ) : null}

            {editing?.audience === "public" ? (
              <Alert role="note" tone="warning">
                {t("endpoints.publicNotice")}
              </Alert>
            ) : null}

            {fitGaps.length > 0 && editedSpace ? (
              <Alert role="note" tone="warning">
                <p>{t("endpoints.surfaceFit.lead", { space: editedSpace })}</p>
                <ul className="list-disc pl-5" data-testid="surface-gaps">
                  {fitGaps.map(({ representation, gap }) => (
                    <li key={representation}>{gap}</li>
                  ))}
                </ul>
                {fitQuery.data?.dataModel ? (
                  <Link
                    to="/projects/$project/models/$name"
                    params={{ project, name: fitQuery.data.dataModel }}
                    className="underline"
                  >
                    {t("endpoints.surfaceFit.openModel", { model: fitQuery.data.dataModel })}
                  </Link>
                ) : (
                  <Link to="/projects/$project/models" params={{ project }} className="underline">
                    {t("endpoints.surfaceFit.addModel")}
                  </Link>
                )}
              </Alert>
            ) : null}

            {/* What the endpoint publishes is its purpose, so the picker is open, never behind a
                click, with what is ticked said above it (T-1389). */}
            {editing?.contextSpaceRef ? (
              <section className="rounded border border-border p-3" aria-labelledby="endpoint-projection">
                <FormHeading id="endpoint-projection" className="text-body font-medium text-fg">
                  {t("endpoints.section.projection")}
                </FormHeading>
                <p data-testid="projection-summary" className="text-caption text-fg-muted">
                  {t("endpoints.projection.summary", {
                    types: Object.values(pickerState.classes).filter((config) => config.ticked).length,
                    attributes: Object.values(pickerState.classes)
                      .filter((config) => config.ticked)
                      .reduce((sum, config) => sum + config.slots.length, 0),
                  })}
                </p>
                <div className="mt-3">
                  <ModelPicker
                    handed={handedClasses}
                    project={project}
                    spaceName={editing.contextSpaceRef}
                    endpointName={editing.name}
                    disabled={propose.isPending}
                    value={pickerState}
                    onChange={setPickerState}
                  />
                </div>
              </section>
            ) : null}

            {/* A new endpoint publishes nothing yet, so there is no schema to hide attributes of. */}
            {!isNew && activeSlug ? (
              <details className="rounded border border-border p-3">
                <summary className="cursor-pointer text-body font-medium text-fg">
                  {t("endpoints.section.hidden")}
                </summary>
                <div className="mt-3">
                  <SchemaProjectionPanel
                    slug={activeSlug}
                    hidden={hidden}
                    onHiddenChange={setHidden}
                    // The projection is proposed as this endpoint's manifest, so it is the
                    // endpoint's verb that decides, and the panel says so on every control
                    // instead of letting a viewer tick boxes for nothing (UI-44, T-1767).
                    disabledReason={
                      permissions.can("Endpoint", "propose")
                        ? undefined
                        : t("permissions.denied", { verb: "propose", kind: "Endpoint" })
                    }
                  />
                </div>
              </details>
            ) : null}

            {/* Beside what it publishes, what it answers: the fastest check of a projection
                (T-1433). A draft endpoint answers nothing yet, so it is offered only for Live. */}
            {!isNew && activeSlug && isLive(base) ? (
              <details className="rounded border border-border p-3">
                <summary className="cursor-pointer text-body font-medium text-fg">
                  {t("endpoints.data.title")}
                </summary>
                <div className="mt-3">
                  <EndpointDataView
                    project={project}
                    slug={activeSlug}
                    hidden={hidden}
                    space={editing?.contextSpaceRef || undefined}
                  />
                </div>
              </details>
            ) : null}

            {/* What the endpoint would serve with the classes and hidden attributes of this form,
                beside the space's original (T-2776, EP-86). Only a Live endpoint answers a preview,
                and only once the projections are known: before, the proof would show the space
                unfiltered under the endpoint's name. */}
            {!isNew && activeSlug && isLive(base) && editing && projectionsQuery.isSuccess ? (
              <details className="rounded border border-border p-3">
                <summary className="cursor-pointer text-body font-medium text-fg">
                  {t("endpoints.proof.title")}
                </summary>
                <div className="mt-3">
                  <FilterProof
                    project={project}
                    name={editing.name}
                    slug={activeSlug}
                    segment={spaceSegment(project, editing.contextSpaceRef, asManifests(spacesQuery.data?.items ?? []))}
                    live
                    draft={formDraft(editing)}
                  />
                </div>
              </details>
            ) : null}

            {previewManifests && editing?.name ? (
              <details className="rounded border border-border p-3">
                <summary className="cursor-pointer text-body font-medium text-fg">
                  {t("endpoints.section.manifests")}
                </summary>
                <CodeBlock
                  label={t("endpoints.section.manifests")}
                  data-testid="endpoint-preview"
                  className="mt-3 max-h-72 rounded border border-border bg-surface p-2.5 font-mono text-xs text-fg-muted"
                >
                  {JSON.stringify(previewManifests, null, 2)}
                </CodeBlock>
              </details>
            ) : null}
          </>
        }
      >
        {isNew ? null : address}
      </ResourceFormDialog>
      )}

      {/* The one MCP address of the platform, behind the header's button (T-6095): it is how a
          person learns the address, and it no longer pushes the list down. */}
      <Dialog
        open={connectOpen}
        onOpenChange={setConnectOpen}
        size="lg"
        title={t("endpoints.hub.title")}
        closeLabel={t("form.cancel")}
      >
        <ConnectAssistant domain={domain} heading={false} />
      </Dialog>
    </div>
  );
}
