import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Badge } from "../../components/ui";
import { useThemeLabel } from "./labels";
import { areaLabel } from "./nuts";
import { formOf } from "./publish";
import type { CatalogBlock } from "./publish";

/**
 * The CKAN page a publication becomes (Architecture/21 §7, step 3; EP-108, EP-143): the dataset's
 * texts and facts, its tags, the resources the publisher writes and the columns the Endpoint
 * serves. The publish flow and the approval page draw the same one, so the approver reads
 * what the steward saw.
 */
export function PublicationPreview({
  title,
  description,
  catalog,
  columns,
  label,
}: {
  title: string;
  description: string;
  catalog: CatalogBlock;
  /** The attributes the public dataset carries; `undefined` when the page does not know them. */
  columns?: string[];
  /** The region's accessible name. */
  label: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const themeLabel = useThemeLabel();
  const form = formOf(catalog, i18n.resolvedLanguage ?? i18n.language ?? "en");
  const keywords = form.keywords.split(",").map((k) => k.trim()).filter(Boolean);
  const area = form.spatial.split(",").map((s) => s.trim()).filter(Boolean).map(areaLabel).join(", ");
  const facts: Array<[string, string]> = [
    [t("catalogue.facets.publisher"), form.publisher],
    [t("catalogue.facets.licence"), form.license ? t(`catalogue.licence.${form.license}`, { defaultValue: form.license }) : ""],
    [t("catalogue.dataset.frequency"), form.frequency ? t(`catalogue.frequency.${form.frequency}`, { defaultValue: form.frequency }) : ""],
    [t("catalogue.facets.spatial"), area],
    [
      t("catalogue.facets.year"),
      form.temporalStart || form.temporalEnd
        ? t("catalogue.dataset.period", {
            start: form.temporalStart || t("catalogue.dataset.open"),
            end: form.temporalEnd || t("catalogue.dataset.open"),
          })
        : "",
    ],
    [t("catalogue.dataset.contact"), [form.contactName, form.contactEmail].filter(Boolean).join(", ")],
  ];
  // What the publisher writes beside the dataset (EP-108, EP-143); the map is the theme's
  // dataset map, drawn wherever a row has a location.
  const resources = [
    ...(columns === undefined || columns.includes("location") ? [t("catalogue.publish.resource.map")] : []),
    t("catalogue.publish.resource.api"),
    t("catalogue.publish.resource.dcat"),
  ];
  return (
    <section aria-label={label} className="flex flex-col gap-3 rounded-md border border-border p-4">
      <p className="text-caption text-fg-muted">{t("catalogue.publish.ckanPage")}</p>
      <h3 className="font-heading text-title font-semibold text-fg">{title}</h3>
      {description ? <p className="text-body text-fg">{description}</p> : null}
      <dl className="grid gap-x-6 gap-y-2 sm:grid-cols-2">
        {facts
          .filter(([, value]) => value.trim() !== "")
          .map(([name, value]) => (
            <div key={name}>
              <dt className="text-caption font-semibold text-fg-muted">{name}</dt>
              <dd className="text-body text-fg">{value}</dd>
            </div>
          ))}
      </dl>
      {form.themes.length > 0 || keywords.length > 0 ? (
        <ul className="flex flex-wrap gap-1" aria-label={t("catalogue.dataset.tags")}>
          {form.themes.map((theme) => (
            <li key={`theme-${theme}`}>
              <Badge tone="info">{themeLabel(theme)}</Badge>
            </li>
          ))}
          {keywords.map((keyword) => (
            <li key={`keyword-${keyword}`}>
              <Badge>{keyword}</Badge>
            </li>
          ))}
        </ul>
      ) : null}
      <h4 className="text-body font-semibold text-fg">{t("catalogue.publish.resources")}</h4>
      <ul className="flex flex-col gap-1" aria-label={t("catalogue.publish.resources")}>
        {resources.map((resource) => (
          <li key={resource} className="rounded border border-border px-2 py-1 text-body text-fg">
            {resource}
          </li>
        ))}
      </ul>
      {columns && columns.length > 0 ? (
        <>
          <h4 className="text-body font-semibold text-fg">{t("catalogue.publish.columns")}</h4>
          <ul className="flex flex-wrap gap-1" aria-label={t("catalogue.publish.columns")}>
            {columns.map((column) => (
              <li key={column}>
                <Badge>
                  <span className="font-mono">{column}</span>
                </Badge>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </section>
  );
}
