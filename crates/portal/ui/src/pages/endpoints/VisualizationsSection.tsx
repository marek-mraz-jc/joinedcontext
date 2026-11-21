import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { useProposal } from "../../api/proposal";
import { door } from "../../api/odata";
import { ChangeNotice } from "../../components/ChangeNotice";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Field,
  Input,
  Select,
  Textarea,
} from "../../components/ui";
import {
  DEFAULT_TEMPORAL,
  GRANULARITIES,
  METHODS,
  RANGES,
  TEMPLATES,
  complete,
  defaultData,
  sourceOf,
  visualizationsOf,
  withVisualizations,
} from "./visualizations";
import type {
  Role,
  TemplateName,
  TemporalDefaults,
  Visualization,
} from "./visualizations";

const DNS_LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

/**
 * The charts CKAN shows on the Endpoint's dataset (T-4642, EP-130, ADR-N-053): an editor picks a
 * template and the columns it places, the Portal fills it from jc-core's template file, and the
 * spec can be edited as JSON before it is proposed. The proposal's dry run is the live check: a
 * spec reaching outside the Endpoint is refused there with its JSON pointer. Readers see the list.
 */
export function VisualizationsSection({
  project,
  manifest,
  types,
  columnsOf,
  served,
  canEdit,
}: {
  project: string;
  manifest: Manifest;
  types: string[];
  columnsOf: (type: string) => string[];
  served: string[];
  canEdit: boolean;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const language = (i18n.resolvedLanguage ?? i18n.language ?? "en").slice(0, 2);
  const list = visualizationsOf(manifest);
  const [change, setChange] = useState<Change | null>(null);
  const proposal = useProposal(project, "endpoints", setChange);
  const [name, setName] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [template, setTemplate] = useState<TemplateName>("line");
  const [picked, setType] = useState("");
  // The model loads after the Endpoint: until a type is picked, the first one listed is the type.
  const type = types.includes(picked) ? picked : (types[0] ?? "");
  const [columns, setColumns] = useState<Partial<Record<Role, string>>>({});
  const [specText, setSpecText] = useState("");
  const [temporal, setTemporal] = useState<TemporalDefaults>(DEFAULT_TEMPORAL);
  const [failed, setFailed] = useState<string | null>(null);

  const propose = (next: Visualization[]) =>
    proposal.mutation.mutate({
      body: withVisualizations(manifest, next) as unknown as ResourceProposal,
      create: false,
    });

  const fill = async () => {
    setFailed(null);
    try {
      const spec = await door("portal_visualization_template", {
        template,
        ...columns,
      });
      setSpecText(JSON.stringify(spec, null, 2));
    } catch (error) {
      setFailed(
        error instanceof ApiError
          ? (error.problem?.detail ?? error.message)
          : String(error),
      );
    }
  };

  const add = () => {
    setFailed(null);
    let spec: Visualization["spec"];
    try {
      spec = JSON.parse(specText) as Visualization["spec"];
    } catch {
      setFailed(t("endpoints.page.visualizations.badJson"));
      return;
    }
    const data = defaultData(template, type, columns.y ?? "", served, temporal);
    if (!data) {
      setFailed(t("endpoints.page.visualizations.noSource"));
      return;
    }
    propose([
      ...list,
      {
        name,
        title: { [language]: title },
        description: { [language]: description },
        data,
        spec,
      },
    ]);
  };

  // A refusal naming one toolbar default marks that field, beside the alert under the form.
  const refused = /\/data\/(last|aggrPeriod|aggrMethod|controls)\b/.exec(
    proposal.error ?? "",
  )?.[1];
  const errorsOf = (member: string) =>
    refused === member && proposal.error ? [proposal.error] : undefined;

  const roles = [
    ...TEMPLATES[template].required,
    ...TEMPLATES[template].optional,
  ] as Role[];
  const nameOk = DNS_LABEL.test(name) && !list.some((one) => one.name === name);
  const ready =
    nameOk &&
    title.trim() !== "" &&
    description.trim() !== "" &&
    specText.trim() !== "";

  return (
    <div className="space-y-3" data-testid="visualizations">
      {list.length === 0 ? (
        <p className="text-body text-fg-muted">
          {t("endpoints.page.visualizations.none")}
        </p>
      ) : (
        <ul className="space-y-2">
          {list.map((one) => (
            <li
              key={one.name}
              className="flex flex-wrap items-center gap-2 rounded-md border border-border p-3"
            >
              <span className="text-sm font-medium text-fg">
                {one.title[language] ?? Object.values(one.title)[0] ?? one.name}
              </span>
              <Badge mono>{one.name}</Badge>
              <Badge>{one.data.source}</Badge>
              {one.data.source === "temporal" && one.data.controls === false ? (
                <Badge>{t("endpoints.page.visualizations.toolbarHidden")}</Badge>
              ) : null}
              {canEdit ? (
                <Button
                  size="sm"
                  variant="secondary"
                  className="ml-auto"
                  disabled={proposal.mutation.isPending}
                  disabledReason={t("app.busyReason")}
                  onClick={() =>
                    propose(list.filter((other) => other.name !== one.name))
                  }
                >
                  {t("endpoints.page.visualizations.remove")}
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      )}

      {canEdit ? (
        <details className="rounded-lg border border-border p-3">
          <summary className="cursor-pointer text-body font-semibold text-fg">
            {t("endpoints.page.visualizations.add")}
          </summary>
          <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
            <Field
              id="viz-name"
              label={t("endpoints.page.visualizations.name")}
              help={t("endpoints.page.visualizations.nameHint")}
              errors={
                name === "" || nameOk
                  ? undefined
                  : [t("endpoints.page.visualizations.nameHint")]
              }
            >
              <Input
                id="viz-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </Field>
            <Field
              id="viz-title"
              label={t("endpoints.page.visualizations.chartTitle")}
            >
              <Input
                id="viz-title"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
              />
            </Field>
            <Field
              id="viz-description"
              label={t("endpoints.page.visualizations.description")}
              className="sm:col-span-2"
            >
              <Input
                id="viz-description"
                value={description}
                onChange={(event) => setDescription(event.target.value)}
              />
            </Field>
            <Field
              id="viz-template"
              label={t("endpoints.page.visualizations.template")}
            >
              <Select
                id="viz-template"
                value={template}
                onChange={(event) => {
                  setTemplate(event.target.value as TemplateName);
                  setSpecText("");
                }}
              >
                {(Object.keys(TEMPLATES) as TemplateName[]).map((one) => (
                  <option key={one} value={one}>
                    {t(`endpoints.page.visualizations.templates.${one}`)}
                  </option>
                ))}
              </Select>
            </Field>
            <Field id="viz-type" label={t("endpoints.field.types")}>
              <Select
                id="viz-type"
                value={type}
                onChange={(event) => {
                  setType(event.target.value);
                  setColumns({});
                }}
              >
                {types.map((one) => (
                  <option key={one} value={one}>
                    {one}
                  </option>
                ))}
              </Select>
            </Field>
            {roles.map((role) => (
              <Field
                key={role}
                id={`viz-${role}`}
                label={t(`endpoints.page.visualizations.roles.${role}`)}
              >
                <Select
                  id={`viz-${role}`}
                  value={columns[role] ?? ""}
                  onChange={(event) =>
                    setColumns({ ...columns, [role]: event.target.value })
                  }
                >
                  <option value="">
                    {t("endpoints.page.visualizations.noColumn")}
                  </option>
                  {columnsOf(type).map((column) => (
                    <option key={column} value={column}>
                      {column}
                    </option>
                  ))}
                </Select>
              </Field>
            ))}
          </div>
          {sourceOf(template, served) === "temporal" ? (
            <fieldset
              className="mt-3 grid grid-cols-1 gap-3 rounded-md border border-border p-3 sm:grid-cols-3"
              data-testid="viz-temporal"
            >
              <legend className="px-1 text-sm font-medium text-fg">
                {t("endpoints.page.visualizations.toolbar")}
              </legend>
              <Field
                id="viz-range"
                errors={errorsOf("last")}
                label={t("endpoints.page.visualizations.range")}
                help={t("endpoints.page.visualizations.hint.range")}
              >
                <Select
                  id="viz-range"
                  value={temporal.range}
                  onChange={(event) =>
                    setTemporal({
                      ...temporal,
                      range: event.target.value as TemporalDefaults["range"],
                    })
                  }
                >
                  {(Object.keys(RANGES) as TemporalDefaults["range"][]).map(
                    (one) => (
                      <option key={one} value={one}>
                        {t(`endpoints.page.visualizations.ranges.${one}`)}
                      </option>
                    ),
                  )}
                </Select>
              </Field>
              <Field
                id="viz-granularity"
                errors={errorsOf("aggrPeriod")}
                label={t("endpoints.page.visualizations.granularity")}
                help={t("endpoints.page.visualizations.hint.granularity")}
              >
                <Select
                  id="viz-granularity"
                  value={temporal.granularity}
                  onChange={(event) =>
                    setTemporal({
                      ...temporal,
                      granularity: event.target
                        .value as TemporalDefaults["granularity"],
                    })
                  }
                >
                  {(
                    Object.keys(GRANULARITIES) as TemporalDefaults["granularity"][]
                  ).map((one) => (
                    <option key={one} value={one}>
                      {t(`endpoints.page.visualizations.granularities.${one}`)}
                    </option>
                  ))}
                </Select>
              </Field>
              {temporal.granularity === "raw" ? null : (
                <Field
                  id="viz-method"
                  errors={errorsOf("aggrMethod")}
                  label={t("endpoints.page.visualizations.method")}
                  help={t("endpoints.page.visualizations.hint.method")}
                >
                  <Select
                    id="viz-method"
                    value={temporal.method}
                    onChange={(event) =>
                      setTemporal({
                        ...temporal,
                        method: event.target.value as TemporalDefaults["method"],
                      })
                    }
                  >
                    {METHODS.map((one) => (
                      <option key={one} value={one}>
                        {t(`endpoints.page.visualizations.methods.${one}`)}
                      </option>
                    ))}
                  </Select>
                </Field>
              )}
              <Field
                id="viz-controls"
                errors={errorsOf("controls")}
                hideLabel
                help={t("endpoints.page.visualizations.hint.controls")}
                className="sm:col-span-3"
              >
                <Checkbox
                  id="viz-controls"
                  label={t("endpoints.page.visualizations.controls")}
                  checked={temporal.controls}
                  onChange={(event) =>
                    setTemporal({ ...temporal, controls: event.target.checked })
                  }
                />
              </Field>
            </fieldset>
          ) : null}
          <Button
            size="sm"
            variant="secondary"
            className="mt-3"
            disabled={!complete(template, columns)}
            disabledReason={t("endpoints.page.visualizations.pickRoles")}
            onClick={() => void fill()}
          >
            {t("endpoints.page.visualizations.fill")}
          </Button>
          <Field
            id="viz-spec"
            label={t("endpoints.page.visualizations.spec")}
            help={t("endpoints.page.visualizations.specHint")}
            className="mt-3"
          >
            <Textarea
              id="viz-spec"
              rows={12}
              className="font-mono text-caption"
              value={specText}
              onChange={(event) => setSpecText(event.target.value)}
            />
          </Field>
          {failed || proposal.error ? (
            <Alert role="alert" tone="danger" className="mt-3">
              {failed ?? proposal.error}
            </Alert>
          ) : null}
          <Button
            className="mt-3"
            disabled={!ready || proposal.mutation.isPending}
            disabledReason={t("endpoints.page.visualizations.incomplete")}
            onClick={add}
          >
            {t("endpoints.page.visualizations.propose")}
          </Button>
        </details>
      ) : null}
      {change ? <ChangeNotice change={change} project={project} /> : null}
    </div>
  );
}
