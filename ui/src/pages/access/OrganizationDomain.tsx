import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, ORG_NAMESPACE } from "../../api/manifest";
import { useChosenOrganizationName } from "../../api/projects";
import type { components } from "../../api/schema";
import { Alert, Badge, Button, Icon } from "../../components/ui";
import type { BadgeTone } from "../../components/ui";

type Verification = components["schemas"]["DomainVerification"];

const TONE: Record<Verification["state"], BadgeTone> = {
  pending: "warning",
  verified: "success",
  failed: "warning",
};

function useOrganizations() {
  return useQuery({
    queryKey: queryKeys.list(ORG_NAMESPACE, "organizations"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "organizations" } },
        }),
      ),
  });
}

/** The TXT record, exactly as it is to be published, and a button that copies exactly that. */
function RecordToPublish({ record }: { record: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1">
      <p className="text-body text-fg">{t("access.domain.publish")}</p>
      <div className="flex flex-wrap items-center gap-2">
        <code data-testid="domain-record" className="break-all rounded-sm bg-surface-subtle px-2 py-1 font-mono text-caption text-fg">
          {record}
        </code>
        <Button
          size="sm"
          icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
          onClick={() => {
            void navigator.clipboard
              ?.writeText(record)
              .then(() => setCopied(true))
              .catch(() => setCopied(false));
          }}
        >
          {copied ? t("access.domain.copied") : t("access.domain.copy")}
        </Button>
      </div>
    </div>
  );
}

/**
 * Project → Access → the organization's domain: whether it is verified, how and when it was
 * checked, and — until it is verified — the TXT record to publish (PF-41, Architecture/03 §3).
 * The challenge is public by design, since it is published in DNS, so anyone who may read the
 * Organization sees it.
 */
export function OrganizationDomain(): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const organizations = useOrganizations();
  // With several Organizations, the one `?org=` chooses (ADR-N-058 section 2.7, T-5248).
  const chosen = useChosenOrganizationName();
  const items = asManifests(organizations.data?.items ?? []).filter(
    (item) => chosen === undefined || item.metadata.name === chosen,
  );
  if (organizations.isPending || (!organizations.error && items.length === 0)) {
    return null;
  }
  const when = (iso: string) =>
    new Date(iso).toLocaleString(i18n.language, { dateStyle: "medium", timeStyle: "short" });

  return (
    <section className="space-y-4" aria-labelledby="domain-heading">
      <div>
        <h2 id="domain-heading" className="text-title font-semibold text-fg">
          {t("access.domain.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("access.domain.lead")}</p>
      </div>
      {organizations.error ? (
        <Alert tone="danger" role="alert">
          {organizations.error instanceof ApiError
            ? (organizations.error.problem?.detail ?? organizations.error.message)
            : t("app.error.generic")}
        </Alert>
      ) : (
        items.map((organization) => {
          const domain = (organization.spec as { domain?: string } | undefined)?.domain ?? "";
          const verification = (organization.status as { domainVerification?: Verification | null } | undefined)
            ?.domainVerification;
          return (
            <div key={organization.metadata.name} className="space-y-2 rounded-md border border-border p-4">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-body text-fg">{domain || organization.metadata.name}</span>
                {verification ? (
                  <Badge tone={TONE[verification.state]}>{t(`access.domain.state.${verification.state}`)}</Badge>
                ) : (
                  <Badge tone="neutral">{t("access.domain.unchecked")}</Badge>
                )}
              </div>
              {verification?.checkedAt ? (
                <p className="text-caption text-fg-muted">
                  {verification.method
                    ? t("access.domain.checkedBy", {
                        method: t(`access.domain.method.${verification.method}`),
                        when: when(verification.checkedAt),
                      })
                    : t("access.domain.checked", { when: when(verification.checkedAt) })}
                </p>
              ) : null}
              {/* Not verified is a step left to do, not an error (T-4990): the steps, the record
                  and, after a failed check, what the check found. */}
              {verification && verification.state !== "verified" ? (
                <Alert tone="warning" title={t("access.domain.steps.title")}>
                  <ol className="list-decimal space-y-2 pl-5">
                    <li>
                      <RecordToPublish record={verification.record} />
                    </li>
                    <li>{t("access.domain.steps.wait")}</li>
                    <li>{t("access.domain.steps.check")}</li>
                  </ol>
                  {verification.state === "failed" && verification.reason ? (
                    <p className="mt-2 text-caption text-fg-muted">
                      {t("access.domain.steps.lastCheck", { reason: verification.reason })}
                    </p>
                  ) : null}
                </Alert>
              ) : null}
            </div>
          );
        })
      )}
    </section>
  );
}
