import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { organizationReading } from "../../api/odata";
import { PageFailed } from "../../components/ui/PageState";
import { EmptyState, safeHref, Skeleton } from "../../components/ui";

/** What answers at an address, in the order the list shows them (API/15 section 2). */
const KINDS = [
  "portal",
  "shared",
  "host",
  "portal-site",
  "opendata",
  "api",
  "mcp",
  "sign-in",
] as const;
type Kind = (typeof KINDS)[number];

/**
 * Every address of the Organizations the person belongs to, as the deployment routes them
 * (T-7013, EP-186): its own hosts, its `/{id}/` path on a shared host, where its people sign in
 * and its MCP servers. Read from the server's host registry; nobody types an address here.
 */
export function OrganizationAddresses(): JSX.Element {
  const { t } = useTranslation();
  const addresses = useQuery({
    queryKey: ["organizationAddresses"],
    queryFn: async () =>
      organizationReading("addresses"),
  });
  return (
    <section
      className="space-y-4"
      aria-labelledby="organization-addresses-heading"
    >
      <div>
        <h2
          id="organization-addresses-heading"
          className="text-title font-semibold text-fg"
        >
          {t("organization.addresses.title")}
        </h2>
        <p className="text-body text-fg-muted">
          {t("organization.addresses.lead")}
        </p>
      </div>
      {addresses.isPending ? (
        <div role="status" className="space-y-2">
          <span className="sr-only">{t("app.loading")}</span>
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      ) : addresses.isError ? (
        <PageFailed
          error={addresses.error}
          onRetry={() => void addresses.refetch()}
        />
      ) : addresses.data.items.length === 0 ? (
        <EmptyState
          title={t("organization.addresses.empty")}
          description={t("organization.addresses.emptyHint")}
        />
      ) : (
        addresses.data.items.map((org) => {
          const sorted = [...org.addresses].sort(
            (a, b) =>
              KINDS.indexOf(a.kind as Kind) - KINDS.indexOf(b.kind as Kind),
          );
          const headingId = `organization-addresses-${org.organization}`;
          return (
            <section
              key={org.organization}
              aria-labelledby={headingId}
              className="space-y-3 rounded-lg border border-border p-4"
            >
              <div>
                <h3 id={headingId} className="text-body font-semibold text-fg">
                  {org.title}
                </h3>
                <p className="text-caption text-fg-muted">
                  {org.organizationId
                    ? t("organization.addresses.id", { id: org.organizationId })
                    : t("organization.addresses.noId")}
                </p>
              </div>
              <ul className="divide-y divide-border">
                {sorted.map((address) => (
                  <li
                    key={`${address.kind}-${address.url}`}
                    className="grid gap-1 py-2 sm:grid-cols-[10rem_1fr] sm:gap-4"
                  >
                    <span className="text-body font-medium text-fg">
                      {t(`organization.addresses.kind.${address.kind}`)}
                    </span>
                    <span className="min-w-0">
                      <a
                        href={safeHref(address.url)}
                        className="focus-ring inline-block py-1 font-mono text-body text-primary-soft-fg underline underline-offset-2 [overflow-wrap:anywhere]"
                        target="_blank"
                        rel="noreferrer"
                      >
                        {address.url}
                      </a>
                      <span className="block text-caption text-fg-muted">
                        {t(`organization.addresses.hint.${address.kind}`)}
                      </span>
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          );
        })
      )}
    </section>
  );
}
