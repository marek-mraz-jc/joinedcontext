/**
 * The organization's data models (DM-75, DM-79), read-only: only a Change to the organization
 * repository edits one, and a project's model uses one by importing it under the name each row
 * shows (DM-76). The list is the server's, which holds none for a person with no binding in the
 * organization. The Data models page of a project shows it below its own models and links to the
 * Organization page's Data models tab, which shows it alone.
 */
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { useOrganizationModels } from "../../components/pickers/organizationModels";
import type { OrganizationModel } from "../../components/pickers/organizationModels";
import { Badge } from "../../components/ui";

/** The name a project's model imports an organization model by (DM-76): `org.{name}.v{major}`. */
export function organizationImportName(name: string, version: string): string {
  return `org.${name}.v${version.split(".")[0]}`;
}

export function OrganizationModels({ inProject = false }: { inProject?: boolean }): JSX.Element {
  const { t } = useTranslation();
  const query = useOrganizationModels();
  const models = (query.data?.items ?? []).filter((model) => model.level === "organization");
  const lifecycle = (value: string) => t(`models.lifecycleOption.${value}`, { defaultValue: value });
  return (
    <section aria-labelledby="models-organization" className="flex flex-col gap-3">
      <div>
        <h2 id="models-organization" className="text-title font-semibold text-fg">
          {t("models.page.organizationSection")}
        </h2>
        <p className="text-body text-fg-muted">{t("models.page.organizationLead")}</p>
        {inProject ? (
          <Link
            to="/organization/$tab"
            params={{ tab: "models" }}
            className="focus-ring text-body text-primary-soft-fg underline hover:no-underline"
          >
            {t("models.page.organizationOpen")}
          </Link>
        ) : null}
      </div>
      <ManifestGrid<OrganizationModel>
        caption={t("models.page.organizationCaption")}
        storageKey="organization-models"
        searchLabel={t("grid.searchIn", { list: t("models.page.organizationSection") })}
        rows={models}
        rowKey={(model) => model.name}
        query={query}
        name={{ id: "name", label: t("models.field.name"), text: (model) => model.name }}
        columns={[
          { id: "version", label: t("models.field.version"), text: (model) => model.version, cell: (model) => <span className="font-mono">{model.version}</span> },
          {
            id: "lifecycle",
            label: t("models.field.lifecycle"),
            filter: true,
            text: (model) => model.lifecycle,
            valueLabel: lifecycle,
            cell: (model) => <Badge tone={model.lifecycle === "published" ? "success" : "neutral"}>{lifecycle(model.lifecycle)}</Badge>,
          },
          { id: "classCount", label: t("models.page.classCount"), numeric: true, align: "right", text: (model) => String(model.classes.length) },
          { id: "classes", label: t("models.classes"), text: (model) => model.classes.join(", ") || "—" },
          {
            id: "importAs",
            label: t("models.page.importAs"),
            text: (model) => organizationImportName(model.name, model.version),
            cell: (model) => <code className="font-mono text-caption">{organizationImportName(model.name, model.version)}</code>,
          },
        ]}
        empty={<p className="text-body text-fg-muted">{t("models.page.organizationEmpty")}</p>}
      />
    </section>
  );
}
