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
  columnsFor,
  complete,
  defaultData,
  refusedSettings,
  refusedSpec,
  sourceOf,
  textAt,
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

/** When each shape is the one to pick. */
const SHAPE_HINT: Record<TemplateName, string> = {
  bar: "barShape",
  line: "lineShape",
  heatmap: "gridShape",
  "small-multiples": "panelsShape",
  "linked-brush": "detailShape",
};
/** What the specification's hint shows as an example. */
const SPEC_EXAMPLE = '"data": {"name": "source"}';
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
  const [picks, setPicks] = useState<Partial<Record<Role, string>>>({});
  const [specText, setSpecText] = useState("");
  const [temporal, setTemporal] = useState<TemporalDefaults>(DEFAULT_TEMPORAL);
  // What Fill wrote last: while the box still holds it, nothing typed by hand is in there.
  const [filled, setFilled] = useState("");
  const [filling, setFilling] = useState(false);
  const [advanced, setAdvanced] = useState(false);
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
    setPicks({});
    setSpecText("");
    setAdvanced(false);
    setTemporal(DEFAULT_TEMPORAL);
    if (editor.current) editor.current.open = false;
  });
  const roles = [...TEMPLATES[template].required, ...TEMPLATES[template].optional] as Role[];
  const source = sourceOf(template, served);
  const offered = (role: Role) => columnsFor(role, source, columnsOf(type));
  // The history has one time column, so it is placed unasked; a pick the chart does not offer is none.
  const columns: Partial<Record<Role, string>> = {};
  for (const role of roles) {
    const column =
      source === "temporal" && role === "x" ? offered(role)[0] : offered(role).find((one) => one === picks[role]);
    if (column) columns[role] = column;
  }
  const propose = (next: Visualization[], at: { added?: number; removed?: Visualization }) => {
    setFailed(null);
    setSent({ charts: next, ...at });
    proposal.mutation.mutate({
      body: withVisualizations(manifest, next) as unknown as ResourceProposal,
      create: false,
    });
  };

  const fill = async (): Promise<string | undefined> => {
    setFailed(null);
    setFilling(true);
    try {
      const spec = await door("portal_visualization_template", {
        template,
        ...columns,
      });
      const text = JSON.stringify(spec, null, 2);
      setSpecText(text);
      setFilled(text);
      return text;
    } catch (error) {
      setFailed(
        error instanceof ApiError
          ? (error.problem?.detail ?? error.message)
          : String(error),
      );
      return undefined;
    } finally {
      setFilling(false);
    }
  };
  // A filled specification names the columns it was filled from: another pick writes it again.
  const repick = (next: Partial<Record<Role, string>>) => {
    setPicks(next);
    if (specText === filled) setSpecText("");
  };

  const add = async () => {
    setFailed(null);
    const text = specText.trim() ? specText : await fill();
    if (!text) return;
    let spec: Visualization["spec"];
    try {
      spec = JSON.parse(text) as Visualization["spec"];
    } catch {
      setFailed(t("endpoints.page.visualizations.badJson"));
      setAdvanced(true);
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
  // An edited specification the check refused, said on its box.
  const badSpec = proposal.error ? refusedSpec(proposal.error) : undefined;
  const ownSpec = badSpec && badSpec.index === sent.added ? badSpec : undefined;
  const address =
    ownSpec && /^(url|href)$/.test(ownSpec.path.at(-1) ?? "")
      ? textAt(sent.charts[ownSpec.index]?.spec, ownSpec.path)
      : undefined;
  const dataLink = address !== undefined && ownSpec?.path.slice(-2).join("/") === "data/url";
  const showRefused = () => {
    const field = document.getElementById(`viz-${refusedField}`);
    field?.focus();
    if (!address || !(field instanceof HTMLTextAreaElement)) return;
    const at = field.value.indexOf(address);
    if (at < 0) return;
    // A data link is replaced whole, member and value.
    const member = dataLink ? field.value.lastIndexOf('"url"', at) : -1;
    field.setSelectionRange(member >= 0 ? member : at, at + address.length + (member >= 0 ? 1 : 0));
  };
  /** Feeds a refused data link from the Endpoint and leaves the rest of the edit as it is. */
  const useSource = () => {
    if (!ownSpec) return;
    try {
      const edit = JSON.parse(specText) as unknown;
      let at = edit as Record<string, unknown> | undefined;
      for (const key of ownSpec.path.slice(0, -2)) at = at?.[key] as Record<string, unknown> | undefined;
      if (at) at.data = { name: "source" };
      setSpecText(JSON.stringify(edit, null, 2));
      proposal.reset();
    } catch {
      setFailed(t("endpoints.page.visualizations.badJson"));
    }
  };
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
  // A range over the limit has an allowed value nearest to it: the chart need not be made again.
  const tooLong =
    blocking && refused.filter((one) => one.index === other?.index).every((one) => one.field === "range");
  const refusal = blocked
    ? sent.removed
      ? t("endpoints.page.visualizations.refusedRemoval", {
          removed: titleOf(sent.removed),
          ...blocked,
        })
      : t("endpoints.page.visualizations.refusedOther", blocked)
    : own.length > 0
      ? t("endpoints.page.visualizations.refusedOwn", { fields: fieldsOf(own) })
      : ownSpec
        ? dataLink
          ? t("endpoints.page.visualizations.refusedLink", { fix: t("endpoints.page.visualizations.useSource") })
          : t("endpoints.page.visualizations.refusedOwn", { fields: t("endpoints.page.visualizations.spec") })
        : proposal.error;
  const refusedField = own.length > 0 ? own[0].field : ownSpec ? "spec" : undefined;
  const alert =
    failed || refusal ? (
      <Alert
        role="alert"
        tone="danger"
        className="mt-3"
        actions={
          failed ? undefined : blocking ? (
            // The remedies, so nobody has to find the right of several Remove buttons.
            <>
              {tooLong ? (
                <Button
                  size="sm"
                  variant="primary"
                  disabled={proposal.mutation.isPending}
                  disabledReason={t("app.busyReason")}
                  onClick={() =>
                    propose(
                      list.map((one) =>
                        one.name === blocking.name ? { ...one, data: { ...one.data, last: RANGES.month } } : one,
                      ),
                      {},
                    )
                  }
                >
                  {t("endpoints.page.visualizations.setRange", {
                    chart: titleOf(blocking),
                    range: t("endpoints.page.visualizations.ranges.month"),
                  })}
                </Button>
              ) : null}
              <Button
                size="sm"
                variant="secondary"
                disabled={proposal.mutation.isPending}
                disabledReason={t("app.busyReason")}
                onClick={() =>
                  propose(list.filter((one) => one.name !== blocking.name), { removed: blocking })
                }
              >
                {t("endpoints.page.visualizations.removeBlocking", { chart: titleOf(blocking) })}
              </Button>
            </>
          ) : refusedField ? (
            <>
              {dataLink ? (
                <Button size="sm" variant="primary" onClick={useSource}>
                  {t("endpoints.page.visualizations.useSource")}
                </Button>
              ) : null}
              <Button size="sm" variant="secondary" onClick={showRefused}>
                {t("endpoints.page.visualizations.showField")}
              </Button>
            </>
          ) : undefined
        }
      >
        {failed ?? refusal}
        {!failed && blocking ? (
          <span className="mt-1 block">
            {sent.removed
              ? t("endpoints.page.visualizations.removeThenRemove", { chart: titleOf(blocking), removed: titleOf(sent.removed) })
              : t("endpoints.page.visualizations.removeFirst", { chart: titleOf(blocking) })}
          </span>
        ) : null}
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

  /** Whether readers may change a temporal chart, and the settings they get. */
  const readersGet = (data: Visualization["data"]) => (
    <p className="w-full text-body text-fg">
      <span className="block text-fg-muted">
        {t(
          data.controls === false
            ? "endpoints.page.visualizations.lockedTo"
            : "endpoints.page.visualizations.startsWith",
        )}
      </span>
      {settingsOf(data)}
    </p>
  );

  // A specification written by hand stands in for the shape and the columns.
  const edited = specText.trim() !== "" && specText !== filled;
  const useFields = () => {
    setSpecText("");
    proposal.reset();
  };
  /** The history's time axis: there is nothing to pick. */
  const fixedAxis = (role: Role) => source === "temporal" && role === "x";
  /** What a role's column is for, in the chart picked, or what to do when there is none. */
  const roleHint = (role: Role) => {
    if (edited && source === "temporal" && role === "y") return t("endpoints.page.visualizations.hint.measureKept");
    if (edited && fixedAxis(role)) return t("endpoints.page.visualizations.hint.axisEdited");
    if (offered(role).length === 0) return t("endpoints.page.visualizations.hint.noColumns");
    if (template === "heatmap") {
      const grid = { x: "gridX", y: "gridY", color: "gridColor", facet: "facet" }[role];
      return t(`endpoints.page.visualizations.hint.${grid}`);
    }
    if (role === "x" && source === "temporal") return t("endpoints.page.visualizations.hint.time");
    if (role === "x" && template === "bar") return t("endpoints.page.visualizations.hint.bars");
    return t(`endpoints.page.visualizations.hint.${role}`);
  };
  const nameOk = DNS_LABEL.test(name) && !list.some((one) => one.name === name);
  const badName = name !== "" && !nameOk;
  const required = TEMPLATES[template].required as readonly Role[];
  // The history is read for its measure, and a specification nobody wrote needs each axis's column.
  const missing = [
    nameOk ? "" : t("endpoints.page.visualizations.name"),
    title.trim() ? "" : t("endpoints.page.visualizations.chartTitle"),
    description.trim() ? "" : t("endpoints.page.visualizations.description"),
    ...required.map((role) =>
      !columns[role] && (!specText.trim() || (source === "temporal" && role === "y"))
        ? t(`endpoints.page.visualizations.roles.${role}`)
        : "",
    ),
  ].filter(Boolean);
  const ready = missing.length === 0;
  const incomplete = t("endpoints.page.visualizations.incomplete", {
    fields: new Intl.ListFormat(language).format(missing),
  });

  return (
    <div className="space-y-3 max-sm:[&_:is(button,summary,legend,p)]:text-title" data-testid="visualizations">
      {awaiting ? (
        <>
          <div className="flex flex-wrap items-center gap-2 rounded-md border border-dashed border-border p-3">
            <span className="text-body font-semibold text-fg max-sm:text-display">
              {t("endpoints.page.visualizations.awaiting", { chart: titleOf(awaiting) })}
            </span>
            <Badge mono>{awaiting.name}</Badge>
            {awaiting.data.source === "temporal" ? (
              <Badge>{t("endpoints.page.visualizations.overTime")}</Badge>
            ) : null}
            {awaiting.data.source === "temporal" ? readersGet(awaiting.data) : null}
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
              <span className="text-body font-semibold text-fg max-sm:text-display">
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
              {one.data.source === "temporal" ? readersGet(one.data) : null}
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
      {canEdit ? null : (
        <p className="text-body text-fg-muted">{t("endpoints.page.visualizations.readOnly")}</p>
      )}

      {canEdit ? (
        <details
          ref={editor}
          // In this form the hints, and what to do on a refusal, are read at body size.
          // The Portal's root is 80 %: at phone width the form reads at the title size and what is
          // typed or picked at the display size, 16 px, which iOS does not zoom into.
          className="rounded-lg border border-border bg-surface p-3 [&_[role=alert]]:break-words sm:[&_[role=alert]]:text-body sm:[&_p]:text-body [&_select:enabled]:bg-surface max-sm:[&:not([open])]:py-0 max-sm:[&_:is(input,select,textarea)]:text-display max-sm:[&_:is(p,label,output,[role=alert])]:text-title"
        >
          <summary className="cursor-pointer content-center text-body font-semibold text-fg">
            {t("endpoints.page.visualizations.add")}
          </summary>
          <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
            <Field
              id="viz-name"
              required
              label={t("endpoints.page.visualizations.name")}
              help={badName ? undefined : t("endpoints.page.visualizations.hint.name")}
              errors={badName ? [t("endpoints.page.visualizations.hint.name")] : undefined}
            >
              <Input
                id="viz-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </Field>
            <Field
              id="viz-title"
              required
              label={t("endpoints.page.visualizations.chartTitle")}
              help={t("endpoints.page.visualizations.hint.title")}
            >
              <Input
                id="viz-title"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
              />
            </Field>
            <Field
              id="viz-description"
              required
              label={t("endpoints.page.visualizations.description")}
              help={t("endpoints.page.visualizations.hint.description")}
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
              required
              label={t("endpoints.page.visualizations.template")}
              help={t(`endpoints.page.visualizations.hint.${SHAPE_HINT[template]}`)}
            >
              <Select
                id="viz-template"
                value={template}
                disabled={edited}
                title={edited ? t("endpoints.page.visualizations.edited") : undefined}
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
            <Field
              id="viz-type"
              required
              label={t("endpoints.page.visualizations.type")}
              help={t(
                types.length
                  ? "endpoints.page.visualizations.hint.type"
                  : "endpoints.page.visualizations.hint.noTypes",
                { example: type },
              )}
            >
              <Select
                id="viz-type"
                value={type}
                onChange={(event) => {
                  setType(event.target.value);
                  repick({});
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
                required={required.includes(role) && !fixedAxis(role)}
                label={t(`endpoints.page.visualizations.roles.${role}`)}
                help={roleHint(role)}
              >
                {fixedAxis(role) ? (
                  <output id={`viz-${role}`} className={`py-1.5 text-body ${edited ? "text-fg-muted" : "text-fg"}`}>
                    {t("endpoints.page.visualizations.readingTime")}
                  </output>
                ) : (
                  <Select
                    id={`viz-${role}`}
                    value={columns[role] ?? ""}
                    // The history is read for its measure, whatever the specification draws.
                    disabled={edited && !(source === "temporal" && role === "y")}
                    title={edited ? t("endpoints.page.visualizations.edited") : undefined}
                    onChange={(event) =>
                      repick({ ...picks, [role]: event.target.value })
                    }
                  >
                    <option value="">
                      {t(
                        required.includes(role)
                          ? "endpoints.page.visualizations.chooseColumn"
                          : "endpoints.page.visualizations.noColumn",
                      )}
                    </option>
                    {offered(role).map((column) => (
                      <option key={column} value={column}>
                        {column}
                      </option>
                    ))}
                  </Select>
                )}
              </Field>
            ))}
          </div>
          {edited ? (
            <Alert
              tone="info"
              className="mt-3"
              actions={
                <Button size="sm" variant="secondary" onClick={useFields}>
                  {t("endpoints.page.visualizations.useFields")}
                </Button>
              }
            >
              {t("endpoints.page.visualizations.edited")}
            </Alert>
          ) : null}
          {source === "temporal" ? (
            <fieldset
              className="mt-3 grid grid-cols-1 gap-3 rounded-md border border-border p-3 sm:grid-cols-3"
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
                      ? temporal.controls
                        ? "endpoints.page.visualizations.hint.rawShown"
                        : "endpoints.page.visualizations.hint.raw"
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
          <details
            className="mt-3"
            open={advanced || ownSpec !== undefined}
            onToggle={(event) => setAdvanced(event.currentTarget.open)}
          >
            <summary className="cursor-pointer content-center text-body font-medium text-fg">
              {t("endpoints.page.visualizations.advanced")}
            </summary>
            {edited ? null : (
              <>
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
              </>
            )}
            <Field
              id="viz-spec"
              label={t("endpoints.page.visualizations.spec")}
              help={t("endpoints.page.visualizations.hint.spec", { example: SPEC_EXAMPLE })}
              errors={
                ownSpec
                  ? [
                    address
                      ? t(`endpoints.page.visualizations.fix.${dataLink ? "dataLink" : "address"}`, { address })
                      : t(`endpoints.page.visualizations.fix.${ownSpec.why}`),
                  ]
                  : undefined
              }
              className="mt-3"
            >
              <Textarea
                id="viz-spec"
                // Short until Fill or a paste puts something to read in it.
                rows={specText ? 12 : 4}
                className="font-mono text-caption"
                value={specText}
                onChange={(event) => setSpecText(event.target.value)}
              />
            </Field>
          </details>
          {removing ? null : alert}
          {ready ? null : (
            <p className="mt-3 text-caption text-fg-muted">
              {incomplete}
            </p>
          )}
          <Button
            // While a repair is offered, the repair is the one primary action.
            variant={dataLink ? "secondary" : "primary"}
            className="mt-3 max-sm:w-full"
            disabled={!ready || filling || proposal.mutation.isPending}
            disabledReason={ready ? t("app.busyReason") : incomplete}
            onClick={() => void add()}
          >
            {t("endpoints.page.visualizations.propose")}
          </Button>
        </details>
      ) : null}
      {change && !awaiting ? <ChangeNotice change={change} project={project} /> : null}
    </div>
  );
}
