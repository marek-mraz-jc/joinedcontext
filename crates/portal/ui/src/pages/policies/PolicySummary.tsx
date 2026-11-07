/**
 * One Policy read in plain words above its fields (T-5591): what it allows as one sentence, then
 * who wrote it down, how long it holds and the last change that made it what it is. The page used
 * to open on the edit form, disabled for a reader, so "what does this Policy allow?" was answered
 * by greyed checkboxes and a folded attribute list.
 */
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import type { PolicyForm } from "../../routes/PoliciesPage";
import { day, policySentence } from "./policySentence";

/** A change that wrote the Policy; a rejected one left it as it was. */
const LANDED = new Set(["Merged", "Applied", "Deploying"]);

function LastChange({ project, name, locale }: { project: string; name: string; locale: string }): JSX.Element {
  const { t } = useTranslation();
  // The same history read the record's own history uses; a person it refuses reads "not recorded".
  // ponytail: first page only (newest first), so a Policy untouched for a page of changes reads
  // "not recorded"; page on with `next` if that shows up.
  const history = useQuery({
    queryKey: [...queryKeys.changes(project), "history", "Policy", name, "latest"] as const,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/history", {
          params: { path: { project }, query: { kind: "Policy", name } },
        }),
      ),
  });
  if (history.isPending) return <>{t("app.loading")}</>;
  const last = (history.data?.items ?? []).find(
    (change) =>
      (change.summary.params as Record<string, unknown>).name === name && LANDED.has(change.status.phase),
  );
  if (!last) return <>{t("policies.summary.notRecorded")}</>;
  const at = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(
    new Date(last.decision?.at ?? last.createdAt),
  );
  return (
    <>
      {last.decision?.by
        ? t("policies.summary.changedApproved", { at, author: last.author.name, approver: last.decision.by })
        : t("policies.summary.changed", { at, author: last.author.name })}
    </>
  );
}

/** `policy` is the stored manifest as the form reads it (fromPolicyEnvelope), so the words and the fields agree. */
export function PolicySummary({ project, policy }: { project: string; policy: PolicyForm }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const from = day(policy.validity?.from, locale);
  const to = day(policy.validity?.to, locale);
  const rules = (policy as { rules?: unknown }).rules;
  const ruleCount = Array.isArray(rules) ? rules.length : 0;
  const facts: [string, JSX.Element | string][] = [
    [t("policies.summary.assigner"), policy.assigner?.trim() ? policy.assigner : t("policies.summary.notRecorded")],
    [t("policies.summary.validFrom"), from ?? t("policies.summary.always")],
    [t("policies.summary.validTo"), to ?? t("policies.summary.noEnd")],
    [t("policies.summary.lastChange"), <LastChange project={project} name={policy.name} locale={locale} />],
  ];
  return (
    <section aria-labelledby="policy-summary" className="flex flex-col gap-3" data-testid="policy-summary">
      <h2 id="policy-summary" className="text-body font-semibold text-fg">
        {t("policies.summary.title")}
      </h2>
      <p className="max-w-prose text-body text-fg" data-testid="policy-summary-sentence">
        {policySentence(policy, t, locale)}
      </p>
      {ruleCount > 0 ? (
        // The form has fields for one rule; a rules[] Policy's grants are edited as YAML (F-UI3-015).
        <p className="max-w-prose rounded-md border border-border bg-surface-subtle p-3 text-body text-fg" data-testid="policy-summary-rules">
          {t("policies.summary.rules", { n: ruleCount })}
        </p>
      ) : null}
      <dl className="grid grid-cols-1 gap-x-6 gap-y-2 text-caption sm:grid-cols-2">
        {facts.map(([term, value]) => (
          <div key={term} className="flex min-w-0 flex-col">
            <dt className="text-fg-muted">{term}</dt>
            <dd className="break-words text-fg">{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
