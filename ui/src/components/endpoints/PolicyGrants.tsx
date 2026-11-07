/**
 * "Who may call it" on an Endpoint's page (T-5592): each Policy that decides the endpoint's calls,
 * read as who may do what to which types and attributes, so "who may write `status`?" is answered
 * here without opening a Policy.
 *
 * Display only, over the Policies the caller could already list: the gateway decides every call.
 * A grantee is named by the title of the group or ServiceAccount the caller may list, and a person
 * by the e-mail the Policy holds; the people directory is never read for it.
 */
import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ORG_NAMESPACE, asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useReadableList } from "../../api/permissions";
import { granteeName, operationAreas } from "../AccessWords";
import { Alert, Badge, Button } from "../ui";
import { PageFailed } from "../ui/PageState";
import { expandOperations, grantChanges, groupOf } from "./operationGroups";
import { policyRules } from "./policyBinding";
import type { Binding } from "./policyBinding";

/** How many policies "Who may call it" shows before "Show all" (T-5015). */
const POLICY_FOLD = 5;

interface Information {
  entities?: { type?: string; id?: string; idPattern?: string }[];
  propertyNames?: string[];
  relationshipNames?: string[];
}

/** One grant of a Policy: the single rule of its spec, or one entry of its `rules` (ADR-N-057). */
interface Rule {
  effect?: string;
  assignee?: { kind?: string; id?: string };
  operations?: string[];
  information?: Information[];
}

/** The grants a Policy holds, each once: `rules[]` when it lists them, else the spec itself. */
export function rulesOf(policy: Manifest): Rule[] {
  return policyRules(policy) as Rule[];
}

/**
 * Title by name, for one kind the caller may list; empty when the list is refused or absent. A
 * list the caller's permissions do not read is never asked for: the API answers it with a 404
 * (T-3852), which the walker reads as a broken page (T-5770).
 */
function useTitles(project: string, plural: string, kind: string, wanted: boolean): Map<string, Manifest> {
  const list = useReadableList(project, plural, kind, wanted);
  const items = asManifests(list.data?.items ?? []);
  return new Map(items.map((item) => [item.metadata.name, item]));
}

export function PolicyGrants({
  project,
  binding,
  pending,
  failure,
}: {
  project: string;
  binding: Binding;
  pending: boolean;
  failure?: { error: unknown; retry: () => void };
}): JSX.Element {
  const { t } = useTranslation();
  const [showAll, setShowAll] = useState(false);
  const policies = binding.kind === "unbound" ? [] : binding.policies;
  const kinds = new Set(policies.flatMap((policy) => rulesOf(policy).map((rule) => rule.assignee?.kind)));
  const titles = {
    group: useTitles(ORG_NAMESPACE, "groups", "Group", kinds.has("group")),
    serviceAccount: useTitles(project, "serviceaccounts", "ServiceAccount", kinds.has("serviceAccount")),
  };

  if (pending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  // A list that failed is not a space with no policies: saying "the grants of the space decide"
  // when the grants could not be read would be the Portal guessing on a security page.
  if (failure) {
    return <PageFailed error={failure.error} onRetry={failure.retry} />;
  }
  if (binding.kind === "unbound") {
    return <Alert tone="warning">{t("endpoints.page.policyUnbound", { urn: binding.urn })}</Alert>;
  }
  if (policies.length === 0) {
    return <p className="text-body text-fg-muted">{t("endpoints.page.noPolicy")}</p>;
  }

  // A space with dozens of policies made this list 9,500 px tall (T-5015): the first few show,
  // the rest one press away.
  const shown = showAll ? policies : policies.slice(0, POLICY_FOLD);
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <ul className="flex flex-col gap-3">
        {shown.map((policy) => (
          <PolicyItem key={policy.metadata.name} project={project} policy={policy} titles={titles} />
        ))}
      </ul>
      {policies.length > POLICY_FOLD ? (
        <Button type="button" variant="secondary" size="sm" className="w-fit" aria-expanded={showAll} onClick={() => setShowAll(!showAll)}>
          {showAll ? t("endpoints.page.policiesFewer") : t("endpoints.page.policiesAll", { count: policies.length })}
        </Button>
      ) : null}
    </div>
  );
}

type Titles = Record<"group" | "serviceAccount", Map<string, Manifest>>;

function PolicyItem({ project, policy, titles }: { project: string; policy: Manifest; titles: Titles }): JSX.Element {
  const { i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  return (
    <li className="min-w-0 rounded-md border border-border p-3">
      <Link
        to="/projects/$project/$plural"
        params={{ project, plural: "policies" }}
        className="focus-ring rounded-sm font-medium text-fg hover:underline [overflow-wrap:anywhere]"
      >
        {localized(policy.metadata.title, locale, policy.metadata.name)}
      </Link>
      <div className="mt-2 flex flex-col gap-3">
        {rulesOf(policy).map((rule, index) => (
          <RuleFacts key={index} rule={rule} titles={titles} />
        ))}
      </div>
    </li>
  );
}

const CHANGE_TONE = { data: "warning", subscriptions: "info", nothing: "success" } as const;

/** One grant: what it changes, who holds it, what it may do, on which types and attributes. */
function RuleFacts({ rule, titles }: { rule: Rule; titles: Titles }): JSX.Element {
  const { t } = useTranslation();
  const operations = rule.operations ?? [];
  const denies = rule.effect === "prohibition";
  const changes = grantChanges(operations);
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <div className="flex flex-wrap gap-2">
        {denies ? (
          <Badge tone="danger">{t("endpoints.page.grantDenies")}</Badge>
        ) : (
          <Badge tone={CHANGE_TONE[changes]}>{t(`endpoints.page.grantChanges.${changes}`)}</Badge>
        )}
      </div>
      <dl className="grid grid-cols-1 gap-x-3 gap-y-1 text-body sm:grid-cols-[max-content_1fr]">
        <dt className="text-caption text-fg-muted">{t("endpoints.page.policyAssignee")}</dt>
        <dd className="min-w-0">
          <Grantee kind={rule.assignee?.kind ?? ""} id={rule.assignee?.id ?? ""} titles={titles} />
        </dd>
        <dt className="text-caption text-fg-muted">{t(denies ? "endpoints.page.grantMayNot" : "endpoints.page.policyGrants")}</dt>
        <dd className="min-w-0">
          <Operations names={operations} />
        </dd>
        <dt className="text-caption text-fg-muted">{t("endpoints.page.grantOn")}</dt>
        <dd className="min-w-0">
          <Covers information={rule.information ?? []} />
        </dd>
      </dl>
    </div>
  );
}

/** The grantee by title or address, its kind as a word, the id the Policy holds on hover. */
function Grantee({ kind, id, titles }: { kind: string; id: string; titles: Titles }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  if (!id) return <span className="text-fg-subtle">{t("policies.sentence.nobody")}</span>;
  const listed = kind === "group" || kind === "serviceAccount" ? titles[kind].get(id) : undefined;
  const name = listed ? localized(listed.metadata.title, locale, id) : granteeName(kind, id, t);
  return (
    <span title={`${kind}:${id}`} className="[overflow-wrap:anywhere]">
      <span className="text-fg">{name}</span>{" "}
      <span className="text-caption text-fg-muted">
        ({t(`choice.principalKind.${kind}`, { defaultValue: kind })})
      </span>
    </span>
  );
}

/**
 * What a grant may do, in words: a group by its label, single operations by their area. The CIM 009
 * names stay one press away, because a group is exactly its members and a reader checking a grant
 * against a manifest needs them (GW34).
 */
function Operations({ names }: { names: string[] }): JSX.Element {
  const { t } = useTranslation();
  if (names.length === 0) return <span className="text-fg-muted">{t("endpoints.page.policyEmpty")}</span>;
  const { areas, other } = operationAreas(names.filter((name) => !groupOf(name)));
  const words = [
    ...names.filter((name) => groupOf(name)).map((name) => t(`choice.operationGroup.${name}`)),
    ...areas.map(({ area }) => t(`access.words.area.${area}`)),
  ];
  const covered = expandOperations(names);
  return (
    <details>
      <summary className="focus-ring cursor-pointer rounded-sm text-fg">{[...new Set(words), ...other].join(", ")}</summary>
      <p className="mt-1 text-caption text-fg-muted">{t("endpoints.page.groupMembers", { count: covered.length })}</p>
      <p className="font-mono text-caption text-fg-muted [overflow-wrap:anywhere]">{covered.join(", ")}</p>
    </details>
  );
}

/** The types a grant reaches and, per type, "all attributes" or the attributes it names (R6, R9). */
export function Covers({ information }: { information: Information[] }): JSX.Element {
  const { t, i18n } = useTranslation();
  const listed = (items: string[]) =>
    new Intl.ListFormat(i18n.language, { style: "long", type: "conjunction" }).format(items);
  if (information.length === 0) return <span>{t("endpoints.page.grantEveryType")}</span>;
  return (
    <ul className="flex flex-col gap-0.5">
      {information.map((item, index) => {
        const types = (item.entities ?? []).map((entity) => entity.type ?? entity.id ?? entity.idPattern ?? "").filter(Boolean);
        const attributes = [...(item.propertyNames ?? []), ...(item.relationshipNames ?? [])];
        return (
          <li key={index} className="[overflow-wrap:anywhere]">
            <span className="font-medium text-fg">{types.length > 0 ? listed(types) : t("endpoints.page.grantAnyType")}</span>
            {": "}
            {attributes.length > 0
              ? t("endpoints.page.grantOnly", { attributes: listed(attributes) })
              : t("endpoints.page.grantAllAttributes")}
          </li>
        );
      })}
    </ul>
  );
}
