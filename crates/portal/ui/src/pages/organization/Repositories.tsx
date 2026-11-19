import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { door } from "../../api/odata";
import { useAdministers } from "../../api/permissions";
import type { components } from "../../api/schema";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { Link } from "@tanstack/react-router";
import { Alert, Badge, Button, buttonClass, EmptyState, Icon } from "../../components/ui";

type Listing = components["schemas"]["OrganizationRepositories"];
type Repository = components["schemas"]["OrganizationRepository"];

/** The list through the Portal's OData door (API/13 §2b). */
async function readRepositories(): Promise<Listing> {
  return (await door("portal_organization_repositories", {})) as Listing;
}

function bytes(size: number, locale: string): string {
  const units = ["B", "KB", "MB", "GB"];
  let value = size;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toLocaleString(locale, { maximumFractionDigits: unit === 0 ? 0 : 1 })} ${units[unit]}`;
}

function when(at: string, locale: string): string {
  const date = new Date(at);
  return at === "" || Number.isNaN(date.getTime()) ? "" : date.toLocaleString(locale, { dateStyle: "medium", timeStyle: "short" });
}

/**
 * Administration → Repositories (T-7409): every forge repository of the Organization, its
 * projects' and their Apps' (and the configuration repository on the installation's
 * Organization), each downloadable as a `.tar.gz` of its default branch. The server lists only
 * this Organization's repositories and refuses anyone but its administrators.
 */
export function Repositories(): JSX.Element {
  const { t } = useTranslation();
  const { known, administers, own } = useAdministers();
  const allowed = administers || own !== undefined;
  if (!known) return <p className="text-body text-fg-muted">{t("orgRepositories.loading")}</p>;
  if (!allowed) return <Alert tone="info">{t("organization.adminOnly")}</Alert>;
  return <RepositoryList />;
}

function RepositoryList(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const listing = useQuery({ queryKey: ["organizationRepositories"], queryFn: readRepositories });
  const rows = listing.data?.repositories ?? [];
  const role = (row: Repository) => t(`orgRepositories.role.${row.role}`);
  const belongs = (row: Repository) =>
    row.app ? t("orgRepositories.ofApp", { app: row.app, project: row.project }) : row.project ? t("orgRepositories.ofProject", { project: row.project }) : t("orgRepositories.ofOrganization");
  return (
    <section className="flex flex-col gap-4" aria-labelledby="repositories-title" data-testid="organization-repositories">
      <div className="flex flex-col gap-1">
        <h2 id="repositories-title" className="text-title font-semibold">
          {t("orgRepositories.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("orgRepositories.lead")}</p>
        {listing.data ? (
          <p className="text-caption text-fg-muted">{t("orgRepositories.hint", { size: bytes(listing.data.maxArchiveBytes, locale) })}</p>
        ) : null}
      </div>
      <AdminGrid<Repository>
        query={listing}
        caption={t("orgRepositories.title")}
        rows={rows}
        rowKey={(row) => row.name}
        storageKey="organization.repositories"
        empty={
          <EmptyState
            bare
            icon="git"
            title={t("orgRepositories.empty")}
            description={t("orgRepositories.emptyHint")}
            action={
              <Link to="/organization/$tab" params={{ tab: "projects" }} className={buttonClass("primary", "md")}>
                {t("orgRepositories.toProjects")}
              </Link>
            }
          />
        }
        stack
        columns={[
          {
            key: "name",
            label: t("orgRepositories.name"),
            text: (row) => row.name,
            cell: (row) => <span className="break-all font-medium">{row.name}</span>,
          },
          {
            key: "role",
            label: t("orgRepositories.what"),
            text: (row) => `${role(row)} ${belongs(row)}`,
            cell: (row) => (
              <span className="flex flex-col gap-0.5">
                <span>{role(row)}</span>
                <span className="text-caption text-fg-muted">{belongs(row)}</span>
              </span>
            ),
          },
          {
            key: "state",
            label: t("orgRepositories.state"),
            text: (row) => (row.archived ? t("orgRepositories.archived") : t("orgRepositories.active")),
            cell: (row) => <Badge tone={row.archived ? "neutral" : "success"}>{row.archived ? t("orgRepositories.archived") : t("orgRepositories.active")}</Badge>,
          },
          { key: "size", label: t("orgRepositories.size"), kind: "number", align: "right", text: (row) => String(row.sizeBytes), cell: (row) => bytes(row.sizeBytes, locale) },
          { key: "updated", label: t("orgRepositories.updated"), kind: "date", text: (row) => row.updatedAt, cell: (row) => when(row.updatedAt, locale) },
          {
            key: "download",
            label: t("orgRepositories.download"),
            actions: true,
            text: () => "",
            cell: (row) =>
              // A repository larger than the limit would stop mid-download: say so before the click.
              listing.data && row.sizeBytes > listing.data.maxArchiveBytes ? (
                <span className="flex flex-col items-start gap-1">
                  <Button variant="secondary" disabled aria-describedby={`too-large-${row.name}`}>
                    {t("orgRepositories.download")}
                  </Button>
                  <span id={`too-large-${row.name}`} className="text-caption text-fg-muted">
                    {t("orgRepositories.tooLarge", { size: bytes(listing.data.maxArchiveBytes, locale) })}
                  </span>
                </span>
              ) : (
                <a
                  className={buttonClass("secondary", "md")}
                  // The REST route itself, so the archive streams to disk instead of passing through
                  // the page's memory as the JSON door would carry it.
                  href={`/api/v1/organization/repositories/${encodeURIComponent(row.name)}/archive`}
                  download
                  aria-label={t("orgRepositories.downloadOne", { name: row.name })}
                >
                  <Icon name="arrowDown" className="size-4" aria-hidden />
                  {t("orgRepositories.download")}
                </a>
            ),
          },
        ]}
      />
    </section>
  );
}
