import { useRef, useState } from "react";
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
  InlineError,
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
  refusedSettings,
  sourceOf,
  visualizationsOf,
  withVisualizations,
} from "./visualizations";
import type {
  RefusedSetting,
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
  // A refusal names a chart by its place in the list that was sent: `added` is the new one's
  // place, `removed` the one a removal left out.
  const [sent, setSent] = useState<{ charts: Visualization[]; added?: number; removed?: Visualization }>({ charts: [] });
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
  const editor = useRef<HTMLDetailsElement>(null);
  const proposal = useProposal(project, "endpoints", (proposed) => {
    setChange(proposed);
    if (sent.added === undefined) return;
    // The chart is proposed: the form starts over, so it is not proposed a second time and the
    // next chart takes none of this one's settings unseen.
    setName("");
    setTitle("");
    setDescription("");
    setTemplate("line");
    setType("");
    setColumns({});
    setSpecText("");
    setTemporal(DEFAULT_TEMPORAL);
    if (editor.current) editor.current.open = false;
  });
  const propose = (next: Visualization[], at: { added?: number; removed?: Visualization }) => {
    setFailed(null);
    setSent({ charts: next, ...at });
    proposal.mutation.mutate({
      body: withVisualizations(manifest, next) as unknown as ResourceProposal,
      create: false,
    });
  };

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
    propose(
      [
        ...list,
        {
          name,
          title: { [language]: title },
          description: { [language]: description },
          data,
          spec,
        },
      ],
      { added: list.length },
    );
  };

  const titleOf = (chart: Visualization) =>
    chart.title[language] ?? Object.values(chart.title)[0] ?? chart.name;

  // Without the toolbar the settings are what readers get, no longer defaults.
  const label = (field: "range" | "granularity" | "method") =>
    t(`endpoints.page.visualizations.${temporal.controls ? "" : "fixed."}${field}`);

  // A refused setting of the chart being added is said on its field; one of a chart already on
  // the endpoint is said by that chart's title. The pointer stays in the API's answer.
  const problem = proposal.mutation.error;
  // The check's findings carry no path: proposeChecked joins their sentences with "; " then.
  const lines = problem instanceof ApiError ? problem.problem?.errors : undefined;
  const refused = proposal.error
    ? refusedSettings(lines?.length ? lines : proposal.error.split(/\n|; /))
    : [];
  const own = refused.filter((one) => one.index === sent.added);
  const other = refused.find((one) => one.index !== sent.added);
  const errorsOf = (field: RefusedSetting["field"]) => {
    if (!own.some((one) => one.field === field)) return undefined;
    if (field === "controls") return [t("endpoints.page.visualizations.fix.controls")];
    // The value that was refused is named and is not among the ones to pick instead.
    const data = sent.charts[sent.added ?? 0].data;
    const [member, group, values] = {
      range: ["last", "ranges", RANGES],
      granularity: ["aggrPeriod", "granularities", GRANULARITIES],
      method: ["aggrMethod", "methods", Object.fromEntries(METHODS.map((one) => [one, one]))],
    }[field] as [string, string, Record<string, string | undefined>];
    const others = Object.keys(values).filter((key) => values[key] !== data[member]);
    return [
      t(`endpoints.page.visualizations.fix.${field}`, {
        value: word(group, values, data[member]),
        choices: new Intl.ListFormat(language, { type: "disjunction" }).format(
          others.map((key) => t(`endpoints.page.visualizations.${group}.${key}`)),
        ),
      }),
    ];
  };
  const fieldsOf = (settings: RefusedSetting[]) =>
    [...new Set(settings.map((one) => (one.field === "controls" ? t("endpoints.page.visualizations.controls") : label(one.field))))].join(", ");
  // A refused field's sentence takes the place of its hint, right under the control.
  const hintOf = (field: RefusedSetting["field"], hint: string) =>
    own.some((one) => one.field === field) ? undefined : hint;
  const blocking = other ? sent.charts[other.index] : undefined;
  const blocked = blocking && {
    chart: titleOf(blocking),
    fields: refused
      .filter((one) => one.index === other?.index)
      .map((one) =>
        t(`endpoints.page.visualizations.${one.field === "controls" ? "controls" : `fixed.${one.field}`}`),
      )
      .join(", "),
  };
  const refusal = blocked
    ? sent.removed
      ? t("endpoints.page.visualizations.refusedRemoval", {
          removed: titleOf(sent.removed),
          ...blocked,
        })
      : t("endpoints.page.visualizations.refusedOther", blocked)
    : own.length > 0
      ? t("endpoints.page.visualizations.refusedOwn", { fields: fieldsOf(own) })
      : proposal.error;
  const alert =
    failed || refusal ? (
      <Alert
        role="alert"
        tone="danger"
        className="mt-3"
        actions={
          !failed && !blocking && own.length > 0 ? (
            <Button
              size="sm"
              variant="secondary"
              onClick={() => document.getElementById(`viz-${own[0].field}`)?.focus()}
            >
              {t("endpoints.page.visualizations.showField")}
            </Button>
          ) : undefined
        }
      >
        {failed ?? refusal}
      </Alert>
    ) : null;
  // A refused removal is said under the list: the editor is closed then.
  const removing = !failed && proposal.error !== null && sent.added === undefined;

  /** A setting in words: whole days are counted, another value outside the lists shows as written. */
  function word(group: string, values: Record<string, string | undefined>, value: unknown) {
    const key = Object.keys(values).find((one) => values[one] === value);
    if (key) return t(`endpoints.page.visualizations.${group}.${key}`);
    const days = /^P(\d+)D$/.exec(String(value))?.[1];
    return days ? t("endpoints.page.visualizations.days", { count: Number(days) }) : String(value);
  }
  /** A temporal source's defaults in words. */
  const settingsOf = (data: Visualization["data"]) => {
    const aggregated = Boolean(data.aggrMethod && data.aggrPeriod);
    const named = (field: string, value: string) =>
      `${t(`endpoints.page.visualizations.fixed.${field}`)}: ${value}`;
    return [
      named("range", word("ranges", RANGES, data.last)),
      named("granularity", word("granularities", GRANULARITIES, aggregated ? data.aggrPeriod : undefined)),
      ...(aggregated
        ? [named("method", word("methods", Object.fromEntries(METHODS.map((one) => [one, one])), data.aggrMethod))]
        : []),
    ].join(" · ");
  };
  const added = change && sent.added !== undefined ? sent.charts[sent.added] : undefined;
  // Proposed and not yet approved: the Endpoint read back does not list it.
  const awaiting = added && !list.some((one) => one.name === added.name) ? added : undefined;

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
      {awaiting ? (
        <>
          <div className="flex flex-wrap items-center gap-2 rounded-md border border-dashed border-border p-3">
            <span className="text-sm font-medium text-fg">{titleOf(awaiting)}</span>
            <Badge mono>{awaiting.name}</Badge>
            {awaiting.data.source === "temporal" ? (
              <Badge>{t("endpoints.page.visualizations.overTime")}</Badge>
            ) : null}
            <Badge>{t("endpoints.page.visualizations.awaiting")}</Badge>
            {awaiting.data.source === "temporal" ? (
              <p className="w-full text-body text-fg">
                {t(
                  awaiting.data.controls === false
                    ? "endpoints.page.visualizations.lockedTo"
                    : "endpoints.page.visualizations.startsWith",
                  { settings: settingsOf(awaiting.data) },
                )}
              </p>
            ) : null}
          </div>
          {change ? <ChangeNotice change={change} project={project} /> : null}
        </>
      ) : null}
      {list.length === 0 ? (
        awaiting ? null : (
          <p className="text-body text-fg-muted">{t("endpoints.page.visualizations.none")}</p>
        )
      ) : (
        <ul className="space-y-2">
          {list.map((one) => (
            <li
              key={one.name}
              className={`flex flex-wrap items-center gap-2 rounded-md border p-3 ${
                one.name === blocking?.name ? "border-danger" : "border-border"
              }`}
            >
              <span className="text-sm font-medium text-fg">
                {titleOf(one)}
              </span>
              <Badge mono>{one.name}</Badge>
              <Badge>
                {one.data.source === "temporal"
                  ? t("endpoints.page.visualizations.overTime")
                  : one.data.source}
              </Badge>
              {canEdit ? (
                <Button
                  size="sm"
                  variant="secondary"
                  className="ml-auto"
                  disabled={proposal.mutation.isPending}
                  disabledReason={t("app.busyReason")}
                  onClick={() =>
                    propose(list.filter((other) => other.name !== one.name), { removed: one })
                  }
                >
                  {t("endpoints.page.visualizations.remove")}
                </Button>
              ) : null}
              {one.data.source === "temporal" ? (
                <p className="w-full text-body text-fg">
                  {t(
                    one.data.controls === false
                      ? "endpoints.page.visualizations.lockedTo"
                      : "endpoints.page.visualizations.startsWith",
                    { settings: settingsOf(one.data) },
                  )}
                </p>
              ) : null}
              {one.name === blocking?.name
                ? refused
                    .filter((at) => at.index === other?.index)
                    .map(({ field }) => (
                      <InlineError key={field} className="w-full [&>span:last-child]:text-body">
                        {t(`endpoints.page.visualizations.invalid.${field}`, {
                          value: word("ranges", RANGES, one.data.last),
                        })}
                      </InlineError>
                    ))
                : null}
            </li>
          ))}
        </ul>
      )}
      {removing ? alert : null}

      {canEdit ? (
        <details ref={editor} className="rounded-lg border border-border p-3">
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
          {complete(template, columns) ? null : (
            <p className="mt-1 text-caption text-fg-muted">
              {t("endpoints.page.visualizations.pickRoles")}
            </p>
          )}
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
          {sourceOf(template, served) === "temporal" ? (
            <fieldset
              // A refusal says what to pick, so it is set at body size.
              className="mt-3 grid grid-cols-1 gap-3 rounded-md border border-border p-3 sm:grid-cols-3 [&_[role=alert]]:text-body"
              data-testid="viz-temporal"
            >
              <legend className="px-1 text-sm font-medium text-fg">
                {t("endpoints.page.visualizations.toolbar")}
              </legend>
              <Field
                id="viz-controls"
                errors={errorsOf("controls")}
                hideLabel
                help={t(
                  temporal.controls
                    ? "endpoints.page.visualizations.hint.controls"
                    : "endpoints.page.visualizations.hint.locked",
                )}
                className="sm:col-span-3"
              >
                <Checkbox
                  id="viz-controls"
                  className="touch-size"
                  label={t("endpoints.page.visualizations.controls")}
                  checked={temporal.controls}
                  onChange={(event) =>
                    setTemporal({ ...temporal, controls: event.target.checked })
                  }
                />
              </Field>
              <Field
                id="viz-range"
                errors={errorsOf("range")}
                label={label("range")}
                help={hintOf(
                  "range",
                  t(
                    temporal.controls
                      ? "endpoints.page.visualizations.hint.range"
                      : "endpoints.page.visualizations.hint.fixedRange",
                  ),
                )}
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
                errors={errorsOf("granularity")}
                label={label("granularity")}
                help={hintOf(
                  "granularity",
                  t(
                    temporal.granularity === "raw"
                      ? "endpoints.page.visualizations.hint.raw"
                      : "endpoints.page.visualizations.hint.granularity",
                  ),
                )}
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
                  errors={errorsOf("method")}
                  label={label("method")}
                  help={hintOf("method", t("endpoints.page.visualizations.hint.method"))}
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
            </fieldset>
          ) : null}
          {removing ? null : alert}
          {ready ? null : (
            <p className="mt-3 text-caption text-fg-muted">
              {t("endpoints.page.visualizations.incomplete")}
            </p>
          )}
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
      {change && !awaiting ? <ChangeNotice change={change} project={project} /> : null}
    </div>
  );
}
