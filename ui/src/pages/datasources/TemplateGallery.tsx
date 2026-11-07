/**
 * The data source templates on the new-source form (T-3249): pick what the source is, give its
 * address, and the form is filled and checked at once, so the first records show before anything
 * is proposed. The full form stays below for everything a template does not ask.
 * Closed at first: one summary line, so the first field shows in the first screen at 400 px (T-6163).
 */
import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Button, Field, Input } from "../../components/ui";
import type { TypedDataSourceType } from "../../schemas/kinds";
import { TEMPLATES, fromTemplate, nameFrom, problemOf } from "./templates";
import type { Template, TemplateProblem } from "./templates";

export interface TemplateGalleryProps {
  /** Fills the form with the template's type and fields, and checks it. */
  onUse: (type: TypedDataSourceType, form: Record<string, unknown>) => void;
}

export function TemplateGallery({ onUse }: TemplateGalleryProps): JSX.Element {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Template | null>(null);
  const [url, setUrl] = useState("");
  const [layer, setLayer] = useState("");
  const [records, setRecords] = useState("");
  const [name, setName] = useState("");
  const [named, setNamed] = useState(false);
  const [problem, setProblem] = useState<TemplateProblem | null>(null);
  const shownName = named ? name : nameFrom(url);
  const errorsFor = (field: TemplateProblem | TemplateProblem[]) =>
    problem && ([] as TemplateProblem[]).concat(field).includes(problem) ? [t(`datasources.template.problem.${problem}`)] : undefined;

  const use = () => {
    if (!picked) return;
    const values = { name: shownName, url, layer, records };
    const found = problemOf(picked, values);
    setProblem(found ?? null);
    if (found) return;
    const filled = fromTemplate(picked, values);
    onUse(filled.type, filled.form);
  };

  return (
    <details className="rounded-md border border-border bg-surface-subtle p-3" data-testid="datasource-templates">
      <summary className="focus-ring cursor-pointer rounded-md text-body font-semibold text-fg">
        {t("datasources.template.title")}
      </summary>
      <p className="mt-1 text-caption text-fg-muted">{t("datasources.template.lead")}</p>
      <ul className="mt-2 grid gap-2 sm:grid-cols-2 lg:grid-cols-3" aria-label={t("datasources.template.title")}>
        {TEMPLATES.map((template) => (
          <li key={template.id}>
            <Button
              variant="secondary"
              aria-pressed={picked?.id === template.id}
              onClick={() => {
                setPicked(template);
                setProblem(null);
              }}
              className={`h-full w-full flex-col items-start gap-1 whitespace-normal py-2 text-left ${
                picked?.id === template.id ? "border-primary-soft-fg bg-primary-soft" : ""
              }`}
            >
              <span className="text-body font-medium text-fg">{t(`datasources.template.${template.id}.title`)}</span>
              <span className="text-caption font-normal text-fg-muted">{t(`datasources.template.${template.id}.what`)}</span>
            </Button>
          </li>
        ))}
      </ul>
      {picked ? (
        <div className="mt-3 flex flex-col gap-2">
          <Field
            id="template-url"
            label={t(`datasources.template.${picked.id}.url`)}
            help={t(`datasources.template.${picked.id}.urlHelp`)}
            errors={errorsFor(["url", "ckanDataset"])}
            required
          >
            <Input id="template-url" type="url" inputMode="url" value={url} onChange={(e) => setUrl(e.target.value)} />
          </Field>
          {picked.asksLayer ? (
            <Field
              id="template-layer"
              label={t("datasources.template.wfs.layer")}
              help={t("datasources.template.wfs.layerHelp")}
              errors={errorsFor("layer")}
            >
              <Input id="template-layer" value={layer} onChange={(e) => setLayer(e.target.value)} />
            </Field>
          ) : null}
          {picked.asksRecords ? (
            <Field
              id="template-records"
              label={t("datasources.field.records")}
              help={t("datasources.field.recordsHelp")}
              errors={errorsFor("records")}
            >
              <Input
                id="template-records"
                placeholder="data.stations"
                value={records}
                onChange={(e) => {
                  setRecords(e.target.value);
                  if (problem === "records") setProblem(null);
                }}
              />
            </Field>
          ) : null}
          <Field
            id="template-name"
            label={t("datasources.field.name")}
            help={t("datasources.template.nameHelp")}
            errors={errorsFor("name")}
            required
          >
            <Input
              id="template-name"
              value={shownName}
              onChange={(e) => {
                setNamed(true);
                setName(e.target.value);
              }}
            />
          </Field>
          <div>
            <Button variant="primary" size="sm" onClick={use}>
              {t("datasources.template.use")}
            </Button>
          </div>
        </div>
      ) : null}
    </details>
  );
}
