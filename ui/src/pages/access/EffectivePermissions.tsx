import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import { operationAreas } from "../../components/AccessWords";
import { DATA_WRITE_OPERATIONS } from "../../components/endpoints/operationGroups";
import { RecordLink } from "../../components/RecordLink";
import { columnLabel } from "../../components/entities/displayOrder";
import { useEndpointSchema } from "../../components/entities/EndpointDataView";
import { classSlots } from "../models/linkml";
import { constraintInWords } from "../policies/constraintWords";
import type { ConstraintName } from "../policies/constraintWords";
import { constraintText } from "../../api/access";
import type { GrantDocument, GrantEntry } from "../../api/access";
import {
  Badge,
  Field,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Term,
} from "../../components/ui";

/** The actions that change context data, by the Portal's one list of them (R8, T-5592). */
export function writesOf(actions: string[]): string[] {
  return actions.filter((action) => DATA_WRITE_OPERATIONS.has(action));
}

export function readsOf(actions: string[]): string[] {
  return actions.filter((action) => !DATA_WRITE_OPERATIONS.has(action));
}

const CONSTRAINT_NAMES: ConstraintName[] = ["q", "scopeQ", "geoQ", "temporalQ"];

export function ConstraintList({ entry, labels }: { entry: GrantEntry; labels: Record<string, string> }): JSX.Element {
  const { t, i18n } = useTranslation();
  const residual = CONSTRAINT_NAMES.flatMap((name) => {
    const value = entry.constraints?.[name];
    return value ? [[name, constraintText(value)] as const] : [];
  });
  if (residual.length === 0) {
    return <span className="text-body text-fg-muted">{t("access.matrix.unconstrained")}</span>;
  }
  // In words, the filter as the Policy writes it on hover and to a screen reader (T-5593).
  return (
    <ul className="space-y-1">
      {residual.map(([name, value]) => (
        <li key={name} title={`${name}=${value}`}>
          {constraintInWords(name, value, t, i18n.language, labels)}
          <span className="sr-only"> ({`${name}=${value}`})</span>
        </li>
      ))}
    </ul>
  );
}

/** A Policy as the space's list returns it: the members a grant entry is compared on. */
interface PolicySource {
  name: string;
  spec: PolicyRuleShape & { contextSpaceRef?: string | { name?: string }; rules?: PolicyRuleShape[] };
}

interface PolicyRuleShape {
  effect?: string;
  information?: { entities?: { type?: string }[]; propertyNames?: string[]; relationshipNames?: string[] }[];
  q?: string;
  scopeQ?: string;
  geoQ?: string;
  temporalQ?: string;
}

function sameAttributes(entry: GrantEntry, names: string[]): boolean {
  if (entry.attributes === "*" || entry.attributes === undefined) return names.length === 0;
  return names.length === entry.attributes.length && entry.attributes.every((name) => names.includes(name));
}

/**
 * The Policies of `space` a grant entry can come from (T-5593): same effect, a selector naming
 * its type, the same attributes and the same four filters; a rule of `rules[]` by its own members,
 * the Policy's where it names none. The document does not name its Policy (the broker keeps that
 * for the Portal's simulator), so this reads the caller's own Policy list; none found says so.
 */
export function sourcesOf(entry: GrantEntry, policies: readonly PolicySource[], space: string, effect: "permission" | "prohibition"): string[] {
  const type = entry.resource?.type ?? "*";
  return policies
    .filter((policy) => {
      const ref = policy.spec.contextSpaceRef;
      if ((typeof ref === "string" ? ref : ref?.name) !== space) return false;
      const rules = policy.spec.rules?.length ? policy.spec.rules : [policy.spec];
      return rules.some((rule) => {
        if ((rule.effect === "prohibition" ? "prohibition" : "permission") !== effect) return false;
        const info = rule.information ?? policy.spec.information ?? [];
        const holding = info.length === 0 ? (type === "*" ? [{}] : []) : info.filter((item) => (item.entities ?? []).some((one) => one.type === type));
        const filters = CONSTRAINT_NAMES.every((name) => (rule[name] ?? policy.spec[name] ?? "") === constraintText(entry.constraints?.[name] ?? ""));
        return filters && holding.some((item) => sameAttributes(entry, [...(item.propertyNames ?? []), ...(item.relationshipNames ?? [])]));
      });
    })
    .map((policy) => policy.name);
}

/** Entries of one type and selector, in the document's order: one row group each (T-5593). */
function byType(entries: GrantEntry[]): GrantEntry[][] {
  const groups = new Map<string, GrantEntry[]>();
  for (const entry of entries) {
    const key = JSON.stringify([entry.resource?.type, entry.resource?.id, entry.resource?.idPatterns]);
    groups.set(key, [...(groups.get(key) ?? []), entry]);
  }
  return [...groups.values()];
}

export function Operations({ actions, tone }: { actions: string[]; tone?: "warning" }): JSX.Element {
  const { t } = useTranslation();
  if (actions.length === 0) return <span className="text-fg-subtle">{t("access.matrix.none")}</span>;
  return (
    <ul className="flex flex-wrap gap-1">
      {/* By area, the CIM 009 names on hover and to a screen reader (T-4990). */}
      {operationAreas(actions).areas.map(({ area, operations }) => (
        <li key={area} title={operations.join(", ")}>
          <Badge tone={tone}>{t(`access.words.area.${area}`)}</Badge>
          <span className="sr-only"> ({operations.join(", ")})</span>
        </li>
      ))}
      {operationAreas(actions).other.map((action) => (
        <li key={action}>
          <Badge mono tone={tone}>
            {action}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

/** Each attribute by the model's label, its name on hover (T-5593). */
function Attributes({ attributes, labels }: { attributes: GrantEntry["attributes"]; labels: Record<string, string> }): JSX.Element {
  const { t, i18n } = useTranslation();
  if (attributes === "*" || attributes === undefined) return <span>{t("access.matrix.allAttributes")}</span>;
  return (
    <ul className="flex flex-wrap gap-1">
      {attributes.map((attribute) => (
        <li key={attribute} title={attribute}>
          <Badge>{labels[attribute] ?? columnLabel(undefined, attribute, i18n.language)}</Badge>
        </li>
      ))}
    </ul>
  );
}

function Sources({ project, names }: { project: string; names: string[] }): JSX.Element {
  const { t } = useTranslation();
  if (names.length === 0) {
    return (
      <span className="text-fg-muted" title={t("access.matrix.noSourceHint")}>
        {t("access.matrix.noSource")}
      </span>
    );
  }
  return (
    <ul className="space-y-1">
      {names.map((name) => (
        <li key={name}>
          <RecordLink project={project} plural="policies" name={name} />
        </li>
      ))}
    </ul>
  );
}

/** Attribute labels of every type the published model of `slug` declares, by type. */
function useLabels(slug: string, language: string): (type: string) => Record<string, string> {
  const { model } = useEndpointSchema(`${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}/schema`, slug);
  return useMemo(() => {
    const parsed = model.data;
    const byType = new Map<string, Record<string, string>>();
    for (const klass of parsed?.classes ?? []) {
      if (!parsed) break;
      byType.set(klass.name, Object.fromEntries(classSlots(parsed, klass).map((slot) => [slot.name, columnLabel(slot, slot.name, language)])));
    }
    return (type: string) => byType.get(type) ?? {};
  }, [model.data, language]);
}

function GrantTable({
  project,
  slug,
  space,
  entries,
  caption,
  effect,
  policies,
}: {
  project: string;
  slug: string;
  space: string;
  entries: GrantEntry[];
  caption: string;
  effect: "permission" | "prohibition";
  policies: readonly PolicySource[];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const labelsOf = useLabels(slug, i18n.language);
  return (
    <Table caption={caption}>
      <TableHead>
        <TableHeaderCell>
          <Term name="entityType">{t("access.matrix.type")}</Term>
        </TableHeaderCell>
        <TableHeaderCell>{t("access.matrix.read")}</TableHeaderCell>
        <TableHeaderCell>{t("access.matrix.write")}</TableHeaderCell>
        <TableHeaderCell>{t("access.matrix.attributes")}</TableHeaderCell>
        <TableHeaderCell>{t("access.matrix.residual")}</TableHeaderCell>
        <TableHeaderCell>{t("access.matrix.source")}</TableHeaderCell>
      </TableHead>
      <TableBody>
        {byType(entries).flatMap((group) => {
          const first = group[0];
          const labels = labelsOf(first.resource?.type ?? "");
          return group.map((entry, index) => {
            const actions = entry.actions ?? [];
            return (
              <TableRow key={`${first.resource?.type ?? "?"}-${entries.indexOf(entry)}`}>
                {index === 0 ? (
                  <th scope="row" rowSpan={group.length} className="px-4 py-3 text-left align-top font-medium text-fg">
                    {first.resource?.type ?? "?"}
                    {first.resource?.idPatterns?.length ? (
                      <div className="font-mono text-caption font-normal text-fg-muted">{first.resource.idPatterns.join(", ")}</div>
                    ) : null}
                    {/* Two grants on one type are two Policies or rules, each its own filters. */}
                    {group.length > 1 ? (
                      <div className="mt-1">
                        <Badge>{t("access.matrix.grants", { count: group.length })}</Badge>
                      </div>
                    ) : null}
                  </th>
                ) : null}
                <TableCell>
                  <Operations actions={readsOf(actions)} />
                </TableCell>
                <TableCell>
                  <Operations actions={writesOf(actions)} tone="warning" />
                </TableCell>
                <TableCell>
                  <Attributes attributes={entry.attributes} labels={labels} />
                </TableCell>
                <TableCell>
                  <ConstraintList entry={entry} labels={labels} />
                </TableCell>
                <TableCell>
                  <Sources project={project} names={sourcesOf(entry, policies, space, effect)} />
                </TableCell>
              </TableRow>
            );
          });
        })}
      </TableBody>
    </Table>
  );
}

/** The caller's own Policy list, to name where each grant comes from; a refusal names none. */
function usePolicySources(project: string): PolicySource[] {
  const policies = useQuery({
    queryKey: queryKeys.list(project, "policies"),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "policies" } },
        }),
      ),
  });
  return asManifests(policies.data?.items ?? []).map((policy) => ({
    name: policy.metadata.name,
    spec: policy.spec as PolicySource["spec"],
  }));
}

/** One endpoint's document: whom it answers, then its grants and its denials. */
function GrantDocumentView({ project, slug, document }: { project: string; slug: string; document: GrantDocument }): JSX.Element {
  const { t } = useTranslation();
  const sources = usePolicySources(project);
  const space = document.resource?.space ?? "";
  const permissions = document.permissions ?? [];
  const prohibitions = document.prohibitions ?? [];
  return (
    <>
      <p className="text-body">
        {t("access.matrix.subject", { subject: document.subject?.id ?? t("access.matrix.anonymous"), space })}
      </p>
      {permissions.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.matrix.empty")}</p>
      ) : (
        <GrantTable
          project={project}
          slug={slug}
          space={space}
          entries={permissions}
          caption={t("access.matrix.title")}
          effect="permission"
          policies={sources}
        />
      )}
      {prohibitions.length > 0 ? (
        <>
          <h3 className="text-title font-medium text-fg">{t("access.matrix.prohibitions")}</h3>
          <GrantTable
            project={project}
            slug={slug}
            space={space}
            entries={prohibitions}
            caption={t("access.matrix.prohibitions")}
            effect="prohibition"
            policies={sources}
          />
        </>
      ) : null}
    </>
  );
}

/**
 * What the signed-in person may do through one endpoint, read from that endpoint's own `/access`
 * surface. The Portal never computes this a second time: the document comes from the PDP that
 * enforces the requests, so the table and the gateway can never disagree (EP-60).
 */
export function EffectivePermissions({
  project,
  endpoint,
  onEndpoint,
}: {
  project: string;
  /** The Endpoint to open on, by name (`?endpoint=`, T-5593); one the list does not hold is ignored. */
  endpoint?: string;
  onEndpoint?: (name: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [slug, setSlug] = useState<string | null>(null);

  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const published = asManifests(endpoints.data?.items ?? []).filter(
    (endpoint) => typeof (endpoint.spec as { slug?: string }).slug === "string",
  );
  // Only an Endpoint the server listed for the caller can be opened by name.
  const named = published.find((one) => one.metadata.name === endpoint) ?? published[0];
  const selected = slug ?? (named?.spec as { slug?: string } | undefined)?.slug ?? null;

  const grants = useQuery({
    queryKey: ["access", project, selected],
    enabled: selected !== null,
    retry: false,
    queryFn: async (): Promise<GrantDocument> => {
      // The gateway's own surface, not a Portal route: one document, one implementation (EP-60).
      const response = await fetch(`/api/endpoint/${encodeURIComponent(selected ?? "")}/access`, {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
      if (!response.ok) {
        throw new ApiError(response.status, `HTTP ${response.status}`);
      }
      return (await response.json()) as GrantDocument;
    },
  });

  return (
    <section className="space-y-4" aria-labelledby="effective-permissions-heading">
      <h2 id="effective-permissions-heading" className="text-title font-semibold text-fg">
        {t("access.matrix.title")}
      </h2>
      <p className="text-body text-fg-muted">{t("access.matrix.hint")}</p>

      {published.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.matrix.noEndpoint")}</p>
      ) : (
        <Field id="effective-permissions-endpoint" label={t("access.matrix.endpoint")} className="max-w-md">
          <Select
            id="effective-permissions-endpoint"
            value={selected ?? ""}
            onChange={(event) => {
              setSlug(event.target.value);
              const name = published.find((one) => (one.spec as { slug?: string }).slug === event.target.value)?.metadata.name;
              if (name) onEndpoint?.(name);
            }}
          >
            {published.map((endpoint) => (
              <option
                key={endpoint.metadata.name}
                value={(endpoint.spec as { slug?: string }).slug}
              >
                {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
              </option>
            ))}
          </Select>
        </Field>
      )}

      {grants.isError ? (
        <p role="status" className="text-body text-fg-muted">
          {grants.error instanceof ApiError && grants.error.status === 403
            ? t("access.matrix.forbidden")
            : t("access.matrix.unavailable")}
        </p>
      ) : null}

      {grants.isPending && selected !== null ? <p role="status">{t("app.loading")}</p> : null}

      {grants.data && selected !== null ? <GrantDocumentView project={project} slug={selected} document={grants.data} /> : null}
    </section>
  );
}
