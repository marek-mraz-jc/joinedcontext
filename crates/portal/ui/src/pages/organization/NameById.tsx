import { useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap, reasonOf } from "../../api/client";
import { ORG_NAMESPACE } from "../../api/manifest";
import { allows, usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import { Alert, Button } from "../../components/ui";

type Plan = components["schemas"]["MigrationPlan"];

/**
 * The one-off move of every repository's people from e-mails to Keycloak ids (ADR-N-061): first
 * the plan (what each repository's Change rewrites, and the addresses nobody carries), then one
 * Change per repository that a second person approves. Offered to whoever updates people.
 */
export function NameById(): JSX.Element | null {
  const { t } = useTranslation();
  const permissions = usePermissions(ORG_NAMESPACE);
  const [plan, setPlan] = useState<Plan | null>(null);
  const [proposed, setProposed] = useState(false);
  const run = useMutation({
    mutationFn: async (dryRun: boolean) =>
      unwrap(
        await api.POST("/api/v1/organization/people/name-by-id", {
          params: { query: { dryRun } },
        }),
      ),
    onSuccess: (answer, dryRun) => {
      setPlan(answer);
      setProposed(!dryRun);
    },
  });
  const rewrites =
    plan?.repositories.reduce(
      (sum, repository) => sum + repository.manifests.length,
      0,
    ) ?? 0;
  if (
    !Array.isArray(permissions.data?.grants) ||
    !allows(permissions.data, "Person", "update")
  )
    return null;
  return (
    <section
      aria-labelledby="name-by-id-heading"
      className="space-y-3 rounded border border-border p-4"
    >
      <h3 id="name-by-id-heading" className="text-body font-semibold text-fg">
        {t("organization.people.nameById.title")}
      </h3>
      <p className="text-body text-fg-muted">
        {t("organization.people.nameById.help")}
      </p>
      {run.error ? (
        <Alert tone="danger" role="alert">
          {reasonOf(run.error)}
        </Alert>
      ) : null}
      {plan ? (
        <div className="space-y-2" role="status">
          {plan.repositories.length === 0 ? (
            <p className="text-body">
              {t("organization.people.nameById.nothing")}
            </p>
          ) : (
            <>
              <p className="text-body">
                {proposed
                  ? t("organization.people.nameById.proposed", {
                      count: plan.repositories.length,
                    })
                  : t("organization.people.nameById.preview", {
                      count: rewrites,
                      repositories: plan.repositories.length,
                    })}
              </p>
              <ul className="space-y-1 text-body">
                {plan.repositories.map((repository) => (
                  <li key={repository.repository}>
                    <span className="font-medium">{repository.repository}</span>
                    {": "}
                    {repository.manifests.join(", ")}
                    {repository.change ? (
                      <>
                        {" ("}
                        <Link
                          to="/projects/$project/approvals/$id"
                          params={{
                            project:
                              repository.repository === "organization"
                                ? ORG_NAMESPACE
                                : repository.repository,
                            id: repository.change,
                          }}
                          className="underline"
                        >
                          {t("organization.people.nameById.change", {
                            change: repository.change,
                          })}
                        </Link>
                        {")"}
                      </>
                    ) : null}
                  </li>
                ))}
              </ul>
            </>
          )}
          {plan.unresolved.length > 0 ? (
            <Alert
              tone="warning"
              title={t("organization.people.nameById.unresolvedTitle")}
            >
              {t("organization.people.nameById.unresolved", {
                addresses: plan.unresolved.join(", "),
              })}
            </Alert>
          ) : null}
        </div>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="secondary"
          loading={run.isPending && run.variables === true}
          disabled={run.isPending}
          disabledReason={t("app.busyReason")}
          onClick={() => run.mutate(true)}
        >
          {t("organization.people.nameById.check")}
        </Button>
        <Button
          size="sm"
          loading={run.isPending && run.variables === false}
          disabled={
            run.isPending ||
            plan === null ||
            proposed ||
            plan.repositories.length === 0
          }
          disabledReason={
            run.isPending
              ? t("app.busyReason")
              : plan === null
                ? t("organization.people.nameById.checkFirst")
                : t("organization.people.nameById.nothingToPropose")
          }
          onClick={() => run.mutate(false)}
        >
          {t("organization.people.nameById.propose")}
        </Button>
      </div>
    </section>
  );
}
