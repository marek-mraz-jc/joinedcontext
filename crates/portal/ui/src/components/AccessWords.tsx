/**
 * Who may do what, in the words a person reads (T-4990): a role's rules as "Propose and approve:
 * 30 kinds" with the kinds one click away, a Policy's CIM 009 operations as "Read entities, Read
 * history", and a grantee as a name rather than `role:endpoint:helsinki/app-air-quality`.
 *
 * Display only. The identifiers stay one step away (a disclosure, a muted line) because a person
 * who has to check a grant against a manifest needs them; the server keeps every check.
 */
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { expandOperations } from "./endpoints/operationGroups";
import { kindPlural } from "../kindWords";

type Translate = (key: string, options?: Record<string, unknown>) => string;

export interface AccessRule {
  kinds?: string[];
  verbs?: string[];
}

/** The areas a CIM 009 operation falls into, in the order a grant reads best. */
export const OPERATION_AREAS = [
  "read",
  "history",
  "subscribe",
  "create",
  "change",
  "delete",
  "registrations",
  "federate",
] as const;
export type OperationArea = (typeof OPERATION_AREAS)[number];

/** The area of one operation name, or `null` for a name none of them covers. */
export function areaOf(operation: string): OperationArea | null {
  if (/EntityMap/.test(operation)) return "federate";
  if (/CSourceRegistration|CSR/.test(operation)) return "registrations";
  if (/Temporal/.test(operation)) return "history";
  if (/Subscription/.test(operation)) return "subscribe";
  if (/^(retrieve|query)/.test(operation)) return "read";
  if (/^create/.test(operation)) return "create";
  if (/^(update|replace|append|merge|upsert|batch)/.test(operation)) return "change";
  if (/^(delete|purge)/.test(operation)) return "delete";
  return null;
}

/** A grant's operations grouped by area, groups expanded; names no area covers are kept apart. */
export function operationAreas(operations: readonly string[]): {
  areas: { area: OperationArea; operations: string[] }[];
  other: string[];
} {
  const byArea = new Map<OperationArea, string[]>();
  const other: string[] = [];
  for (const operation of expandOperations(operations)) {
    const area = areaOf(operation);
    if (area) byArea.set(area, [...(byArea.get(area) ?? []), operation]);
    else other.push(operation);
  }
  return {
    areas: OPERATION_AREAS.filter((area) => byArea.has(area)).map((area) => ({
      area,
      operations: byArea.get(area) ?? [],
    })),
    other,
  };
}

/** `Propose and approve`, in the page's language. */
export function verbsInWords(verbs: readonly string[], t: Translate, language: string): string {
  const words = verbs.map((verb, index) => {
    const word = t(`access.words.verb.${verb}`, { defaultValue: verb });
    return index === 0 ? word : word.toLocaleLowerCase(language);
  });
  return new Intl.ListFormat(language, { style: "long", type: "conjunction" }).format(words);
}

/**
 * The display name of a grantee: an endpoint's role by its endpoint, anything else by its id.
 * `endpoint:{project}/{endpoint}[/{tier}]` is the role an Endpoint declares for its readers and,
 * with a tier, for its stewards.
 */
export function granteeName(kind: string, id: string, t: Translate): string {
  const endpointRole = kind === "role" ? /^endpoint:[^/]+\/([^/]+)(?:\/(.+))?$/.exec(id) : null;
  if (endpointRole) {
    const [, endpoint, tier] = endpointRole;
    return tier
      ? t("access.words.endpointTier", { endpoint, tier })
      : t("access.words.endpointRole", { endpoint });
  }
  return id;
}

/** Up to this many kinds are named in the line itself; more go behind "Show the N kinds". */
const INLINE_KINDS = 3;

/** One rule: "Propose and approve: context spaces, mappings" or "…: 30 kinds" with the list. */
export function RuleInWords({ rule }: { rule: AccessRule }): JSX.Element {
  const { t, i18n } = useTranslation();
  const verbs = verbsInWords(rule.verbs ?? [], t, i18n.language);
  const kinds = rule.kinds ?? [];
  const named = kinds.map((kind) => (kind === "*" ? t("access.words.everyKind") : kindPlural(kind, t)));
  if (kinds.length <= INLINE_KINDS) {
    return (
      <p>
        <span className="font-medium text-fg">{verbs}</span>
        {": "}
        {named.join(", ") || t("access.words.noKind")}
      </p>
    );
  }
  return (
    <details className="group">
      <summary className="focus-ring cursor-pointer rounded-sm">
        <span className="font-medium text-fg">{verbs}</span>
        {": "}
        {t("access.words.kinds", { count: kinds.length })}
      </summary>
      <p className="mt-1 text-caption text-fg-muted">{named.join(", ")}</p>
    </details>
  );
}

/** A role's rules, one line each, the preset's name first when the role is one. */
export function RulesInWords({ rules, preset }: { rules: AccessRule[]; preset?: string | null }): JSX.Element {
  const { t } = useTranslation();
  if (rules.length === 0) return <span className="text-fg-subtle">{t("access.words.nothing")}</span>;
  return (
    <div className="flex flex-col gap-1">
      {preset ? <p className="font-semibold text-fg">{t(`access.presets.${preset}.label`)}</p> : null}
      {rules.map((rule, index) => (
        <RuleInWords key={index} rule={rule} />
      ))}
    </div>
  );
}

/** A Policy's operations by area, the CIM 009 names behind "Show its N operations". */
export function OperationsInWords({ operations }: { operations: readonly string[] }): JSX.Element {
  const { t } = useTranslation();
  if (operations.length === 0) return <span className="text-fg-subtle">{t("access.words.nothing")}</span>;
  const { areas, other } = operationAreas(operations);
  const covered = expandOperations(operations);
  const words = [...areas.map(({ area }) => t(`access.words.area.${area}`)), ...other];
  return (
    <details>
      <summary className="focus-ring cursor-pointer rounded-sm text-fg">{words.join(", ")}</summary>
      <p className="mt-1 font-mono text-caption text-fg-muted [overflow-wrap:anywhere]">
        {covered.join(", ")}
      </p>
    </details>
  );
}

/** A grantee by name, what kind of grantee it is muted beside it and its id on hover. */
export function GranteeInWords({ kind, id }: { kind: string; id: string }): JSX.Element {
  const { t } = useTranslation();
  if (!id) return <span className="text-fg-subtle">{t("policies.sentence.nobody")}</span>;
  return (
    <span className="flex flex-col" title={`${kind}:${id}`}>
      <span className="text-fg">{granteeName(kind, id, t)}</span>
      <span className="text-caption text-fg-muted">
        {t(`choice.principalKind.${kind}`, { defaultValue: kind })}
      </span>
    </span>
  );
}
