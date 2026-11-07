import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Alert, Button, ConfirmDialog, Icon, InlineError, Input } from "../../components/ui";

type AccountAccessAnswer = components["schemas"]["AccountAccess"];
type IssuedSecret = components["schemas"]["IssuedSecret"];

/** One read-only value of the issued secret with its copy button. */
function CopyField({ id, label, value }: { id: string; label: string; value: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1">
      <label htmlFor={id} className="block text-caption font-medium text-fg">
        {label}
      </label>
      <div className="flex flex-wrap items-center gap-2">
        <Input id={id} readOnly value={value} className="min-w-0 flex-1 font-mono" onFocus={(e) => e.currentTarget.select()} />
        <Button
          variant="secondary"
          size="sm"
          aria-label={t("access.secret.copyNamed", { what: label })}
          icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
          onClick={() => {
            void navigator.clipboard
              ?.writeText(value)
              .then(() => setCopied(true))
              .catch(() => setCopied(false));
          }}
        >
          {copied ? t("access.secret.copied") : t("access.secret.copy")}
        </Button>
      </div>
    </div>
  );
}

/**
 * The client secret of an account an external system signs in as (PF-37, T-5626): a person who
 * may approve ServiceAccounts issues a new one, which this panel shows once and forgets on Hide;
 * the Portal keeps no copy. An account with a workload or a key pair has no secret, and says why.
 */
export function AccountSecret({ project, account }: { project: string; account: string }): JSX.Element | null {
  const { t } = useTranslation();
  const [asking, setAsking] = useState(false);
  // The same answer the Access section reads: one request for both.
  const access = useQuery({
    queryKey: [...queryKeys.list(project, "serviceaccounts"), account, "access"],
    queryFn: async (): Promise<AccountAccessAnswer> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/serviceaccounts/{name}/access", {
          params: { path: { project, name: account } },
        }),
      ),
  });
  const issue = useMutation({
    mutationFn: async (): Promise<IssuedSecret> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/serviceaccounts/{name}/secret", {
          params: { path: { project, name: account } },
        }),
      ),
    // The secret lives only in this mutation: Done resets it, and an unmounted panel's copy is
    // dropped from the cache at once rather than after react-query's five minutes.
    gcTime: 0,
    onSuccess: () => setAsking(false),
  });
  const issued = issue.data;

  if (!access.data) return null;
  const { noSecret, mayIssueSecret } = access.data;
  const clientId = `${project}-${account}`;
  const heading = `secret-${account}`;

  return (
    <section className="space-y-3" aria-labelledby={heading}>
      <h4 id={heading} className="font-medium text-fg">
        {t("access.secret.title")}
      </h4>
      {noSecret ? (
        <p className="text-body text-fg-muted">{t(`access.secret.none.${noSecret}`)}</p>
      ) : (
        <>
          <p className="text-body text-fg-muted">{t("access.secret.hint", { clientId })}</p>
          {issue.isError ? (
            <InlineError>{issue.error instanceof Error ? issue.error.message : t("access.secret.failed")}</InlineError>
          ) : null}
          {issued ? (
            <Alert
              tone="warning"
              title={t("access.secret.onceTitle")}
              actions={
                <Button variant="secondary" size="sm" onClick={() => issue.reset()}>
                  {t("access.secret.done")}
                </Button>
              }
            >
              <p className="mb-3">{t("access.secret.once")}</p>
              <div className="space-y-3">
                <CopyField id={`${heading}-client`} label={t("access.secret.clientId")} value={issued.clientId} />
                <CopyField id={`${heading}-value`} label={t("access.secret.secret")} value={issued.clientSecret} />
                <CopyField id={`${heading}-url`} label={t("access.secret.tokenUrl")} value={issued.tokenUrl} />
              </div>
            </Alert>
          ) : null}
          {mayIssueSecret ? (
            <Button variant="secondary" onClick={() => setAsking(true)}>
              {t("access.secret.issue")}
            </Button>
          ) : (
            <p className="text-caption text-fg-muted">{t("access.secret.whoIssues")}</p>
          )}
        </>
      )}
      <ConfirmDialog
        open={asking}
        onOpenChange={(open) => setAsking(open)}
        title={t("access.secret.confirmTitle", { account })}
        description={t("access.secret.confirmHelp")}
        confirmLabel={t("access.secret.issue")}
        pending={issue.isPending}
        onConfirm={() => issue.mutate()}
      />
    </section>
  );
}
