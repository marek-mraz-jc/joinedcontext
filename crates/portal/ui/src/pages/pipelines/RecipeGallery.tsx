/**
 * The recipes on a new pipeline (T-3258): pick what the pipeline does, give its source and where it
 * writes, and the form below is filled; the workbench then samples the source, drafts the mapping
 * and tests it before anything is proposed.
 */
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { localized, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { AttributePicker } from "../../components/pickers/AttributePicker";
import { TypePicker } from "../../components/pickers/TypePicker";
import { Button, Field, Select } from "../../components/ui";
import type { PipelineForm } from "./PipelineEditor";
import { RECIPES, fromRecipe, problemOf } from "./recipes";
import type { Recipe, RecipeProblem } from "./recipes";

export interface RecipeGalleryProps {
  project: string;
  orgDomain?: string;
  dataSources: Manifest[];
  endpoints: Manifest[];
  /** The Endpoints a load may write through, as the form offers them. */
  targets: { name: string; urn?: string }[];
  onUse: (form: PipelineForm) => void;
  /** Opens the KPI editor over several sources (PL-105); absent hides its button. */
  onSeveralSources?: () => void;
  /** The project's own templates, listed under the recipes (T-3976). */
  children?: ReactNode;
}

export function RecipeGallery({ project, orgDomain, dataSources, endpoints, targets, onUse, onSeveralSources, children }: RecipeGalleryProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const [picked, setPicked] = useState<Recipe | null>(null);
  const [source, setSource] = useState("");
  const [target, setTarget] = useState("");
  const [type, setType] = useState("");
  const [attribute, setAttribute] = useState("");
  const [problem, setProblem] = useState<RecipeProblem | null>(null);
  const errors = (field: RecipeProblem) => (problem === field ? [t(`pipelines.recipe.problem.${field}`)] : undefined);
  // An App's own Endpoint (app-*) is the App's door, never a pipeline's source (T-6572).
  const sources = picked?.source === "endpoint" ? endpoints.filter((one) => !one.metadata.name.startsWith("app-")) : dataSources;
  // The KPI reads an Endpoint: its space's model lists the types and their numbers.
  const sourceSpace = refName(endpoints.find((one) => one.metadata.name === source)?.spec.contextSpaceRef) || undefined;

  const use = () => {
    if (!picked) return;
    const values = { source, target, type, attribute };
    const found = problemOf(picked, values);
    setProblem(found ?? null);
    if (!found) onUse(fromRecipe(picked, values, { project, endpoints, orgDomain }));
  };

  return (
    <details open className="rounded-md border border-border bg-surface-subtle p-3" data-testid="pipeline-recipes">
      <summary className="focus-ring cursor-pointer rounded-md text-body font-semibold text-fg">
        {t("pipelines.recipe.title")}
      </summary>
      <p className="mt-1 text-caption text-fg-muted">{t("pipelines.recipe.lead")}</p>
      <ul className="mt-2 grid gap-2 sm:grid-cols-2" aria-label={t("pipelines.recipe.title")}>
        {RECIPES.map((recipe) => (
          <li key={recipe.id}>
            <Button
              variant="secondary"
              aria-pressed={picked?.id === recipe.id}
              onClick={() => {
                if (picked?.source !== recipe.source) setSource("");
                setPicked(recipe);
                setProblem(null);
              }}
              className={`h-full w-full flex-col items-start gap-1 whitespace-normal py-2 text-left ${
                picked?.id === recipe.id ? "border-primary-soft-fg bg-primary-soft" : ""
              }`}
            >
              <span className="text-body font-medium text-fg">{t(`pipelines.recipe.${recipe.id}.title`)}</span>
              <span className="text-caption font-normal text-fg-muted">{t(`pipelines.recipe.${recipe.id}.what`)}</span>
            </Button>
          </li>
        ))}
      </ul>
      {picked ? (
        <div className="mt-3 flex flex-col gap-2">
          <Field
            id="recipe-source"
            label={t(`pipelines.recipe.${picked.id}.source`)}
            help={sources.length === 0 ? t(`pipelines.recipe.none.${picked.source}`) : undefined}
            errors={errors("source")}
            required
          >
            <Select
              id="recipe-source"
              value={source}
              onChange={(event) => {
                setSource(event.target.value);
                setType("");
                setAttribute("");
              }}
            >
              <option value="">{t("pipelines.recipe.choose")}</option>
              {sources.map((one) => (
                <option key={one.metadata.name} value={one.metadata.name}>
                  {sourceLabel(one, i18n.language)}
                </option>
              ))}
            </Select>
          </Field>
          {picked.id === "kpi" ? (
            <>
              <Field
                id="recipe-type"
                label={t("pipelines.recipe.kpi.type")}
                help={t("pipelines.recipe.kpi.typeHelp")}
                errors={errors("type")}
                required
              >
                <TypePicker
                  id="recipe-type"
                  label={t("pipelines.recipe.kpi.type")}
                  labelled
                  project={project}
                  space={sourceSpace}
                  value={type ? [type] : []}
                  onChange={(types) => {
                    setType(types[0] ?? "");
                    setAttribute("");
                  }}
                />
              </Field>
              <Field
                id="recipe-attribute"
                label={t("pipelines.recipe.kpi.attribute")}
                help={t("pipelines.recipe.kpi.attributeHelp")}
                errors={errors("attribute")}
                required
              >
                <AttributePicker
                  id="recipe-attribute"
                  label={t("pipelines.recipe.kpi.attribute")}
                  labelled
                  project={project}
                  space={sourceSpace}
                  type={type || undefined}
                  kind="Property"
                  numeric
                  value={attribute ? [attribute] : []}
                  onChange={(names) => setAttribute(names[0] ?? "")}
                />
              </Field>
            </>
          ) : (
            <Field
              id="recipe-target"
              label={t("pipelines.recipe.target")}
              help={t("pipelines.recipe.targetHelp")}
              errors={errors("target")}
              required
            >
              <Select id="recipe-target" value={target} onChange={(event) => setTarget(event.target.value)}>
                <option value="">{t("pipelines.recipe.choose")}</option>
                {targets
                  .filter((one) => one.urn)
                  .map((one) => (
                    <option key={one.urn} value={one.urn}>
                      {one.name}
                    </option>
                  ))}
              </Select>
            </Field>
          )}
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" size="sm" onClick={use}>
              {t("pipelines.recipe.use")}
            </Button>
            {picked.id === "kpi" && onSeveralSources ? (
              <Button variant="secondary" size="sm" onClick={onSeveralSources}>
                {t("pipelines.recipe.kpi.several")}
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
      {children}
    </details>
  );
}

/** A source by its title and its name, so two feeds of one publisher read apart (T-6572). */
export function sourceLabel(source: Manifest, language: string): string {
  const name = source.metadata.name;
  const title = localized(source.metadata.title, language, name);
  return title === name ? name : `${title} (${name})`;
}
