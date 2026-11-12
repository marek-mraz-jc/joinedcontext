/**
 * The Fields tab of a model's page (T-5066, DM-61, DM-74): every field of every type with what the
 * model allows (type, required, list, allowed values, bounds, pattern, unit, description) beside
 * what the model's space holds now (how many entities fill it, the range of its numbers, from the
 * last daily quality run) and a link that opens Explore on the entities carrying it.
 *
 * The layout follows the attribute tables of the Smart Data Models pages and the LinkML generated
 * docs: one table per type, one row per field, the constraints in words. The live facts come from
 * `GET …/spaces/{space}/quality`, which leaves ranges out for a person who may not read the space,
 * so a viewer sees what their rights show and nothing more.
 */
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { localized } from "../../api/manifest";
import { Badge } from "../../components/ui";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { fieldWord, useFieldWords } from "../../components/entities/fieldTypes";
import { classSlots, isAnyClass, unitCode, valueClasses } from "./linkml";
import type { LinkmlModel, LinkmlSlot } from "./linkml";

type Report = components["schemas"]["SpaceQuality"];
type TypeReport = components["schemas"]["TypeQuality"];

function isReport(value: unknown): value is Report {
  return typeof value === "object" && value !== null && "observedAt" in value;
}

/** What the model allows a field to hold, as short phrases: the enum's values, bounds, pattern, unit. */
export function allowedOf(slot: LinkmlSlot, model: LinkmlModel, language: string): string[] {
  const out: string[] = [];
  const values = model.enums.find((one) => one.name === slot.range)?.permissible_values ?? [];
  if (values.length > 0) {
    out.push(values.map((value) => (value.title ? `${value.name} (${localized(value.title, language, value.name)})` : value.name)).join(", "));
  }
  if (slot.minimum_value !== undefined || slot.maximum_value !== undefined) {
    out.push(`${slot.minimum_value ?? "…"} – ${slot.maximum_value ?? "…"}`);
  }
  if (slot.pattern) out.push(slot.pattern);
  const unit = slot.unit?.symbol ?? slot.unit?.ucum_code ?? unitCode(slot.unit);
  if (unit) out.push(unit);
  return out;
}

export function ModelFields({ project, model, space }: { project: string; model: LinkmlModel; space?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const word = useFieldWords();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  // The same query key as the space page's report and Explore's Quality tab: one read serves all.
  const quality = useQuery({
    queryKey: ["projects", project, "spaces", space, "quality"],
    enabled: space !== undefined,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/quality", {
          params: { path: { project, space: space ?? "" } },
        }),
      ) as unknown,
  });
  const report = isReport(quality.data) ? quality.data : undefined;
  const percent = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 });
  const number = new Intl.NumberFormat(locale, { maximumFractionDigits: 3 });
  const hidden = valueClasses(model);
  const types = model.classes.filter((klass) => !isAnyClass(klass) && !hidden.has(klass.name));

  const status =
    space === undefined
      ? t("models.fields.noSpace")
      : quality.isPending
        ? t("app.loading")
        : quality.isError
          ? t("models.fields.factsFailed", { reason: quality.error instanceof Error ? quality.error.message : "" })
          : report === undefined
            ? t("models.fields.notYet", { space })
            : t("models.fields.observed", {
                space,
                at: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(report.observedAt)),
              });

  // The share of the space's entities that carry the field: what the column sorts by.
  const shareOf = (own: TypeReport | undefined, name: string): number | undefined =>
    report === undefined || own === undefined || own.count === 0
      ? undefined
      : (own.attributes.find((one) => one.name === name)?.present ?? 0) / own.count;
  const facts = (own: TypeReport | undefined, name: string): JSX.Element | string => {
    if (report === undefined) return "—";
    if (own === undefined || own.count === 0) return t("models.fields.noEntities");
    const attribute = own.attributes.find((one) => one.name === name);
    const share = shareOf(own, name) ?? 0;
    // The rules the run found broken on this field, e.g. MinInclusive on pm10.
    const broken = report.rules.filter((rule) => rule.path === name);
    return (
      <span className="flex flex-col gap-0.5">
        <span>
          <Badge tone={share >= 0.99 ? "success" : share >= 0.8 ? "warning" : share > 0 ? "danger" : "neutral"}>{percent.format(share)}</Badge>{" "}
          <span className="text-caption text-fg-muted">
            {t("explore.quality.present", { present: attribute?.present ?? 0, count: own.count })}
          </span>
        </span>
        {attribute?.min !== undefined && attribute.min !== null && attribute.max !== undefined && attribute.max !== null ? (
          <span className="text-caption text-fg-muted">
            {t("models.fields.seenRange", { min: number.format(attribute.min), max: number.format(attribute.max) })}
          </span>
        ) : null}
        {broken.length > 0 ? (
          <span className="text-caption text-danger">
            {t("models.fields.broken", {
              count: broken.reduce((sum, rule) => sum + rule.count, 0),
              rules: broken.map((rule) => rule.rule).join(", "),
            })}
          </span>
        ) : null}
      </span>
    );
  };

  return (
    <section aria-labelledby="model-fields" className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 id="model-fields" className="text-sm font-semibold">
          {t("models.fields.title")}
        </h2>
        <p className="text-body text-fg-muted" role={quality.isError ? "alert" : undefined}>
          {status}
        </p>
      </div>
      {types.length === 0 ? <p className="text-body text-fg-muted">{t("models.fields.noTypes")}</p> : null}
      {types.map((klass) => {
        const title = localized(klass.title, i18n.language, klass.name);
        const own = report?.types.find((one) => one.type === klass.name);
        const fields = classSlots(model, klass);
        return (
          <div key={klass.name} className="flex min-w-0 flex-col gap-2">
            <h3 className="text-body font-semibold [overflow-wrap:anywhere]">
              {title}
              {title !== klass.name ? <span className="ml-2 font-mono text-caption text-fg-muted">{klass.name}</span> : null}
              {own !== undefined ? (
                <span className="ml-2 text-caption font-normal text-fg-muted">{t("models.fields.count", { count: own.count })}</span>
              ) : null}
            </h3>
            {klass.description ? <p className="text-caption text-fg-muted">{klass.description}</p> : null}
            {fields.length === 0 ? (
              <p className="text-body text-fg-muted">{t("models.page.noFields")}</p>
            ) : (
              <AdminGrid<LinkmlSlot>
                caption={t("models.fields.caption", { type: title })}
                storageKey="models:fields"
                stack
                rows={fields}
                rowKey={(slot) => slot.name}
                columns={[
                  {
                    key: "slot",
                    label: t("models.slot"),
                    text: (slot) => localized(slot.title, i18n.language, slot.name),
                    cell: (slot) => {
                      const label = localized(slot.title, i18n.language, slot.name);
                      return (
                        <>
                          {label}
                          {label !== slot.name ? <span className="block font-mono text-caption text-fg-muted">{slot.name}</span> : null}
                        </>
                      );
                    },
                  },
                  {
                    key: "range",
                    label: t("models.range"),
                    text: (slot) =>
                      [
                        word(fieldWord(slot, model)),
                        slot.required ? t("models.required") : "",
                        slot.multivalued ? t("models.multivalued") : "",
                      ]
                        .filter(Boolean)
                        .join(" · "),
                    cell: (slot) => (
                      <>
                        {word(fieldWord(slot, model))}
                        {slot.required ? <span className="text-fg-muted"> · {t("models.required")}</span> : null}
                        {slot.multivalued ? <span className="text-fg-muted"> · {t("models.multivalued")}</span> : null}
                      </>
                    ),
                  },
                  {
                    key: "allowed",
                    label: t("models.fields.allowed"),
                    text: (slot) => allowedOf(slot, model, i18n.language).join(", "),
                    cell: (slot) => {
                      const allowed = allowedOf(slot, model, i18n.language);
                      return allowed.length === 0 ? (
                        // A link's target type is in the Type column already.
                        <span className="text-fg-muted">{fieldWord(slot, model).type === "relationship" ? "—" : t("models.fields.anyValue")}</span>
                      ) : (
                        <span className="flex flex-col gap-0.5 [overflow-wrap:anywhere]">
                          {allowed.map((one) => (
                            <span key={one}>{one}</span>
                          ))}
                        </span>
                      );
                    },
                  },
                  {
                    key: "description",
                    label: t("models.fields.description"),
                    text: (slot) => slot.description ?? "—",
                    cell: (slot) => <span className="block max-w-prose text-caption">{slot.description ?? "—"}</span>,
                  },
                  {
                    key: "inSpace",
                    label: t("models.fields.inSpace"),
                    kind: "number",
                    filter: false,
                    text: (slot) => String(shareOf(own, slot.name) ?? ""),
                    cell: (slot) => facts(own, slot.name),
                  },
                  {
                    key: "query",
                    label: t("models.fields.query"),
                    actions: true,
                    text: () => "",
                    cell: (slot) =>
                      space !== undefined ? (
                        <Link
                          to="/projects/$project/explore"
                          params={{ project }}
                          search={{ space, type: klass.name, q: slot.name }}
                          aria-label={t("models.fields.queryLabel", { type: title, field: localized(slot.title, i18n.language, slot.name) })}
                          className="focus-ring text-primary-soft-fg underline underline-offset-2"
                        >
                          {t("models.fields.queryAction")}
                        </Link>
                      ) : (
                        "—"
                      ),
                  },
                ]}
                empty={<p className="text-body text-fg-muted">{t("models.page.noFields")}</p>}
              />
            )}
          </div>
        );
      })}
    </section>
  );
}
