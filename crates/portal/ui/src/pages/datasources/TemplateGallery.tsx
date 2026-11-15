/**
 * The new data source wizard (T-3249, T-7094 G-22), the first thing the new-source form shows:
 * pick what the source is, give its address and, when it needs one, the secret its credential
 * sits in; an open data API then lists what it offers, the first answer tells how it pages, and
 * Fill the form and check fills the form below and checks it at once, so the first records show
 * before anything is proposed. A source the wizard does not walk through (a stream, a server, a
 * database) opens its own form below, which stays there for everything as the advanced way.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { Button, Checkbox, Field, Input, Select } from "../../components/ui";
import type { DataSourceType, TypedDataSourceType } from "../../schemas/kinds";
import { PresetDiscovery } from "./PresetDiscovery";
import { TEMPLATES, fromTemplate, nameFrom, pagingUrl, pagingWords, problemOf } from "./templates";
import type { PagingGuess, Template, TemplateProblem } from "./templates";

/** A source the wizard hands to its own form: a stream, a file server, a database. */
export interface OtherSource {
  type: DataSourceType;
  label: string;
}

export interface TemplateGalleryProps {
  project: string;
  /** The secrets this project's sources already use, the only credentials the wizard offers. */
  secrets: string[];
  /** The other connections the page's form has, each opened in it. */
  others: OtherSource[];
  /** Fills the form with the template's type and fields, and checks it. */
  onUse: (type: TypedDataSourceType, form: Record<string, unknown>) => void;
  /** Opens the form of another type. */
  onPick: (type: DataSourceType) => void;
}

const NO_SECRET = "";

export function TemplateGallery({ project, secrets, others, onUse, onPick }: TemplateGalleryProps): JSX.Element {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Template | null>(null);
  const [url, setUrl] = useState("");
  const [layer, setLayer] = useState("");
  const [records, setRecords] = useState("");
  const [rows, setRows] = useState(true);
  const [allPages, setAllPages] = useState(true);
  const [secretName, setSecretName] = useState(NO_SECRET);
  const [secretKey, setSecretKey] = useState("");
  const [http, setHttp] = useState<Record<string, unknown> | undefined>(undefined);
  const [name, setName] = useState("");
  const [named, setNamed] = useState(false);
  const [problem, setProblem] = useState<TemplateProblem | null>(null);
  const shownName = named ? name : nameFrom(url);
  const errorsFor = (field: TemplateProblem | TemplateProblem[]) =>
    problem && ([] as TemplateProblem[]).concat(field).includes(problem) ? [t(`datasources.template.problem.${problem}`)] : undefined;
  // An open data API's block as the discovery reads and writes it: the typed URL until a pick.
  const presetHttp = picked?.preset ? (http ?? { url: url.trim(), preset: { kind: picked.preset } }) : undefined;
  const values = () => ({
    name: shownName,
    url,
    layer,
    records,
    rows,
    http: presetHttp,
    paging: allPages ? (guess.data?.paging as Record<string, unknown> | undefined) : undefined,
    secret: secretName === NO_SECRET ? undefined : { name: secretName, key: secretKey },
  });

  const guess = useMutation({
    mutationFn: async (asked: string) =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/datasource-paging", {
          params: { path: { project } },
          body: { url: asked },
        }),
      )) as PagingGuess,
    onSuccess: (answer) => {
      // The records path the first answer names, unless the person already typed one.
      if (answer.records && records.trim() === "") setRecords(answer.records);
    },
  });

  const choose = (template: Template) => {
    setPicked(template);
    setProblem(null);
    setHttp(undefined);
    guess.reset();
  };

  const use = () => {
    if (!picked) return;
    const found = problemOf(picked, values());
    setProblem(found ?? null);
    if (found) return;
    const filled = fromTemplate(picked, values());
    onUse(filled.type, filled.form);
  };

  const findPaging = () => {
    if (!picked) return;
    const found = problemOf(picked, { ...values(), secret: undefined });
    setProblem(found === "url" || found === "ckanDataset" ? found : null);
    if (found === "url" || found === "ckanDataset") return;
    try {
      guess.mutate(pagingUrl(picked, values()));
    } catch {
      setProblem("url");
    }
  };

  const guessFailure =
    guess.error instanceof ApiError
      ? (guess.error.problem?.detail ?? guess.error.message)
      : guess.error
        ? t("app.error.generic")
        : null;
  const words = guess.data ? pagingWords(guess.data) : null;
  const pages = picked?.group === "api" || picked?.asksRecords;
  const card = (template: Template) => {
    const title = template.preset
      ? t(`datasources.preset.kinds.${template.preset}`)
      : t(`datasources.template.${template.id}.title`);
    const what = template.preset ? t(`datasources.wizard.api.${template.preset}`) : t(`datasources.template.${template.id}.what`);
    return (
      <li key={template.id}>
        <Button
          variant="secondary"
          aria-pressed={picked?.id === template.id}
          onClick={() => choose(template)}
          className={`h-full w-full flex-col items-start gap-1 whitespace-normal py-2 text-left ${
            picked?.id === template.id ? "border-primary-soft-fg bg-primary-soft" : ""
          }`}
        >
          <span className="text-body font-medium text-fg">{title}</span>
          <span className="text-caption font-normal text-fg-muted">{what}</span>
        </Button>
      </li>
    );
  };
  // The steps are numbered as they show: the discovery only for an API, the format only for a file.
  let at = 1;
  const step = (label: string, id?: string) => (
    <h3 id={id} className="text-body font-semibold text-fg">
      {t("datasources.wizard.step", { n: ++at, label })}
    </h3>
  );

  return (
    <details open className="rounded-md border border-border bg-surface-subtle p-3" data-testid="datasource-templates">
      <summary className="focus-ring cursor-pointer rounded-md text-body font-semibold text-fg">
        {t("datasources.template.title")}
      </summary>
      <p className="mt-1 text-caption text-fg-muted">{t("datasources.template.lead")}</p>
      <section aria-labelledby="wizard-kind" className="mt-2 flex flex-col gap-2">
        <h3 id="wizard-kind" className="text-body font-semibold text-fg">
          {t("datasources.wizard.step", { n: 1, label: t("datasources.wizard.kind") })}
        </h3>
        {(["api", "file"] as const).map((group) => (
          <div key={group} className="flex flex-col gap-1">
            <p className="text-caption font-medium text-fg-muted">{t(`datasources.wizard.groups.${group}`)}</p>
            <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3" aria-label={t(`datasources.wizard.groups.${group}`)}>
              {TEMPLATES.filter((template) => template.group === group).map(card)}
            </ul>
          </div>
        ))}
        {others.length > 0 ? (
          <Field
            id="wizard-other"
            label={t("datasources.wizard.groups.other")}
            help={t("datasources.wizard.otherHelp")}
          >
            <Select
              id="wizard-other"
              value=""
              onChange={(event) => {
                if (event.target.value) onPick(event.target.value);
              }}
            >
              <option value="">{t("datasources.wizard.otherNone")}</option>
              {others.map((other) => (
                <option key={other.type} value={other.type}>
                  {other.label}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
      </section>
      {picked ? (
        <div className="mt-3 flex flex-col gap-3">
          <section className="flex flex-col gap-2">
            {step(t("datasources.wizard.address"))}
            <Field
              id="template-url"
              label={picked.preset ? t("datasources.wizard.apiUrl") : t(`datasources.template.${picked.id}.url`)}
              help={picked.preset ? t(`datasources.wizard.apiUrlHelp.${picked.preset}`) : t(`datasources.template.${picked.id}.urlHelp`)}
              errors={errorsFor(["url", "ckanDataset"])}
              required
            >
              <Input
                id="template-url"
                type="url"
                inputMode="url"
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value);
                  setHttp(undefined);
                  guess.reset();
                }}
              />
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
            {picked.type === "http" ? (
              <div className="flex flex-wrap gap-2">
                <Field
                  id="template-secret"
                  label={t("datasources.wizard.secret")}
                  help={secrets.length > 0 ? t("datasources.wizard.secretHelp") : t("datasources.wizard.secretNone")}
                >
                  <Select
                    id="template-secret"
                    value={secretName}
                    onChange={(event) => {
                      setSecretName(event.target.value);
                      if (problem === "secretKey") setProblem(null);
                    }}
                  >
                    <option value={NO_SECRET}>{t("datasources.wizard.secretPublic")}</option>
                    {secrets.map((secret) => (
                      <option key={secret} value={secret}>
                        {secret}
                      </option>
                    ))}
                  </Select>
                </Field>
                {secretName !== NO_SECRET ? (
                  <Field
                    id="template-secret-key"
                    label={t("datasources.field.secretKey")}
                    help={t("datasources.wizard.secretKeyHelp")}
                    errors={errorsFor("secretKey")}
                    required
                  >
                    <Input id="template-secret-key" value={secretKey} placeholder="token" onChange={(e) => setSecretKey(e.target.value)} />
                  </Field>
                ) : null}
              </div>
            ) : null}
          </section>
          {picked.preset && presetHttp ? (
            <section className="flex flex-col gap-2">
              {step(t("datasources.wizard.discover"))}
              <PresetDiscovery
                project={project}
                http={presetHttp}
                onUse={(next) => {
                  setHttp(next);
                  guess.reset();
                }}
              />
            </section>
          ) : null}
          <section className="flex flex-col gap-2" aria-labelledby="wizard-paging">
            {step(t("datasources.wizard.pagingTitle"), "wizard-paging")}
            {pages ? (
              <>
                <p className="text-caption text-fg-muted">{t("datasources.wizard.pagingHint")}</p>
                <div>
                  <Button type="button" variant="secondary" size="sm" loading={guess.isPending} onClick={findPaging}>
                    {guess.isPending ? t("datasources.wizard.pagingRunning") : t("datasources.wizard.pagingButton")}
                  </Button>
                </div>
                <div role="status" aria-live="polite" className="flex flex-col gap-1" data-testid="wizard-paging">
                  {guessFailure ? <p className="text-body font-medium text-danger">{guessFailure}</p> : null}
                  {words ? (
                    <p className={`text-body ${guess.data?.skipped ? "text-danger" : ""}`}>
                      {t(`datasources.wizard.paging.${words.key}`, words.values)}
                    </p>
                  ) : null}
                  {picked.asksRecords && guess.data?.paging ? (
                    <Checkbox
                      label={t("datasources.wizard.allPages")}
                      hint={t("datasources.wizard.allPagesHint")}
                      checked={allPages}
                      onChange={(event) => setAllPages(event.target.checked)}
                    />
                  ) : null}
                  {picked.preset && guess.data && !guess.data.skipped ? (
                    <p className="text-caption text-fg-muted">{t("datasources.wizard.presetPages")}</p>
                  ) : null}
                </div>
              </>
            ) : (
              <p className="text-caption text-fg-muted">{t("datasources.wizard.oneRead")}</p>
            )}
          </section>
          {picked.group === "file" && (picked.id === "csv" || picked.asksRecords) ? (
            <section className="flex flex-col gap-2">
              {step(t("datasources.wizard.format"))}
              {picked.id === "csv" ? (
                <Checkbox
                  label={t("datasources.wizard.rows")}
                  hint={t("datasources.wizard.rowsHint")}
                  checked={rows}
                  onChange={(event) => setRows(event.target.checked)}
                />
              ) : (
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
              )}
            </section>
          ) : null}
          <section className="flex flex-col gap-2">
            {step(t("datasources.wizard.check"))}
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
            {secretName !== NO_SECRET ? <p className="text-caption text-fg-muted">{t("datasources.wizard.secretProbe")}</p> : null}
            <div>
              <Button variant="primary" size="sm" onClick={use}>
                {t("datasources.template.use")}
              </Button>
            </div>
          </section>
        </div>
      ) : null}
    </details>
  );
}
