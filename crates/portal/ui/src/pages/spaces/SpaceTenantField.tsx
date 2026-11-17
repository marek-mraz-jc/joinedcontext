import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Fragment } from "react";
import { spaceTenantQuery, usageRefusal } from "../../api/spaceUsage";
import { Alert } from "../../components/ui/Alert";

/**
 * The broker Tenant of a space, shown and never edited (T-7216, ARCH/TEN-10): the platform
 * derives it, so the page names it as a term and its value, says what it is for, and says why
 * it is not known yet instead of an error page.
 */
export function SpaceTenantField({ project, space }: { project: string; space: string }): React.JSX.Element {
  const { t } = useTranslation();
  const tenant = useQuery(spaceTenantQuery(project, space));
  const value = tenant.data?.tenant;
  const status = tenant.isPending
    ? t("app.loading")
    : tenant.isError
      ? t("spaces.inside.brokerTenant.failed", { reason: usageRefusal(tenant.error) })
      : value
        ? undefined
        : t("spaces.inside.brokerTenant.unknown", { reason: tenant.data?.reason ?? "" });
  return (
    <dl className="max-w-xl space-y-1">
      <dt className="text-body font-medium text-fg">{t("spaces.inside.brokerTenant.label")}</dt>
      <dd className="space-y-1">
        {status === undefined ? (
          // Breaks only after a `__`, so a phone never splits an id inside itself.
          <code data-testid="space-tenant" className="block select-all break-words rounded-md border border-border bg-surface px-3 py-2 font-mono text-body text-fg">
            {(value ?? "").split("__").map((part, index) => (
              <Fragment key={index}>
                {index > 0 ? (
                  <>
                    __
                    <wbr />
                  </>
                ) : null}
                {part}
              </Fragment>
            ))}
          </code>
        ) : tenant.isPending ? (
          <p role="status" data-testid="space-tenant-status" className="text-body text-fg-muted">
            {status}
          </p>
        ) : (
          <Alert tone={tenant.isError ? "danger" : "warning"} data-testid="space-tenant-status">
            {status}
          </Alert>
        )}
        <p className="text-caption text-fg-muted">{t("spaces.inside.brokerTenant.hint")}</p>
      </dd>
    </dl>
  );
}
