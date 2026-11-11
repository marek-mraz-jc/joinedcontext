import { useContext } from "react";
import type { JSX } from "react";
import type { FieldProps } from "@rjsf/utils";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../../api/client";
import { asManifests, refName } from "../../../api/manifest";
import { Button, Card, EmptyState, Field, InlineError, Input, Select, Textarea } from "../../ui";
import { LIST_BODY, LIST_FIELDSET, ListLegend } from "../theme";
import { FormDataContext } from "./EntitySelectorField";
import { ListFailed } from "./ListFailed";
import { FormProjectContext } from "./ModelWidgets";
import { FeedSourceEditor, IcalSourceEditor, MapSourceEditor } from "./OutputSourceEditors";
import {
  DEFAULT_ZONE,
  duplicateNames,
  entriesOf,
  ICAL_SORTS,
  isSlug,
  limitFits,
  MAX_FEED_LIMIT,
  MAX_ICAL_LIMIT,
  MAX_OUTPUTS,
  newOutput,
  outputsOf,
  queryOf,
  timeZones,
  withMember,
  withQuery,
  withText,
  written,
} from "./representationOutputs";
import type { FeedOutput, FeedSource, Format, IcalOutput, MapSource, Output, Outputs, WidgetOutput } from "./representationOutputs";
import type { IcalMapping } from "./icalMapping";

const LANGUAGES = ["sk", "en"] as const;

/**
 * `spec.representations`: the Endpoint's named calendar and feed outputs (API/11 EP-159, owner
 * 2026-10-22T09:30Z). Each format is a list a person adds to, edits and removes from; each output
 * has a name that becomes its address, a title per language, an optional query that narrows the
 * view, and one or more model classes with the attribute of each role picked from the space's
 * model. Both lists start empty: a feed or a calendar is offered only where items appear over time.
 * The save checks every member against the model and names the one it refuses.
 */
export function RepresentationOutputsField(props: FieldProps): JSX.Element {
  const { formData, onChange, disabled, readonly, schema, fieldPathId } = props;
  const { t } = useTranslation();
  const id = fieldPathId.$id;
  const project = useContext(FormProjectContext) ?? "";
  const root = useContext(FormDataContext) as { name?: unknown; slug?: unknown; contextSpaceRef?: unknown; enabledRepresentations?: unknown } | undefined;
  // a preview reads through the saved view: an Endpoint saved once has its slug
  const endpoint = typeof root?.name === "string" && typeof root.slug === "string" && root.slug ? { project, name: root.name } : undefined;
  const icalServed = Array.isArray(root?.enabledRepresentations) && root.enabledRepresentations.includes("ical");
  const space = typeof root?.contextSpaceRef === "string" ? root.contextSpaceRef : refName(root?.contextSpaceRef as string | undefined) ?? "";
  const outputs = outputsOf(formData);
  const frozen = Boolean(disabled || readonly);

  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "datamodels" } } })),
    enabled: Boolean(project),
  });
  const model = asManifests(models.data?.items ?? []).find(
    (m) => refName((m.spec as { contextSpaceRef?: unknown } | undefined)?.contextSpaceRef as string | undefined) === space,
  );
  const write = (next: Outputs): void => onChange(written(formData, next), fieldPathId.path);
  const common = { project, space, model, frozen };

  return (
    <fieldset id={id} data-testid="representation-outputs" className={LIST_FIELDSET}>
      <ListLegend heading={schema.title ?? ""} />
      <div className={`${LIST_BODY} flex flex-col gap-6`}>
        <p id={`${id}__help`} className="text-caption text-fg-muted">
          {t("endpoints.outputs.help")}
        </p>
        {models.isError ? (
          <ListFailed what={t("nav.models")} reason={reasonOf(models.error, t("app.error"))} onRetry={() => void models.refetch()} />
        ) : null}
        {!models.isLoading && !models.isError && !model ? <p className="text-caption text-warning">{t("endpoints.ical.noModel", { space })}</p> : null}
        <OutputList
          id={`${id}-ical`}
          format="ical"
          outputs={outputs.ical}
          off={icalServed ? undefined : t("endpoints.outputs.icalOff")}
          onChange={(ical) => write({ ...outputs, ical: ical as IcalOutput[] })}
          endpoint={endpoint}
          renderSource={(sourceId, source, change) => (
            <IcalSourceEditor id={sourceId} value={source as IcalMapping} onChange={change} {...common} />
          )}
          frozen={frozen}
        />
        <OutputList
          id={`${id}-feed`}
          format="feed"
          outputs={outputs.feed}
          onChange={(feed) => write({ ...outputs, feed: feed as FeedOutput[] })}
          renderSource={(sourceId, source, change) => (
            <FeedSourceEditor id={sourceId} value={source as FeedSource} onChange={change} {...common} />
          )}
          frozen={frozen}
        />
        <OutputList
          id={`${id}-widget`}
          format="widget"
          outputs={outputs.widget}
          onChange={(widget) => write({ ...outputs, widget: widget as WidgetOutput[] })}
          renderSource={(sourceId, source, change) => (
            <MapSourceEditor id={sourceId} value={source as MapSource} onChange={change} {...common} />
          )}
          frozen={frozen}
        />
      </div>
    </fieldset>
  );
}

type AnyOutput = Output<IcalMapping | FeedSource | MapSource>;
type AnySource = IcalMapping | FeedSource | MapSource;

interface OutputListProps {
  id: string;
  format: Format;
  outputs: AnyOutput[];
  /** Why the list cannot be edited now (the representation is off), shown instead of it. */
  off?: string;
  onChange: (next: AnyOutput[]) => void;
  renderSource: (id: string, source: AnySource, onChange: (next: AnySource) => void) => JSX.Element;
  frozen: boolean;
  /** The saved Endpoint a calendar is previewed through; absent before the first save. */
  endpoint?: { project: string; name: string };
}

/** One format's outputs: a card per output, an add button, and what to do when there is none. */
function OutputList({ id, format, outputs, off, onChange, renderSource, frozen, endpoint }: OutputListProps): JSX.Element {
  const { t } = useTranslation();
  const k = `endpoints.outputs.${format}`;
  const twice = duplicateNames(outputs);
  const full = outputs.length >= MAX_OUTPUTS;
  const add = (): void => {
    const type = outputs[0]?.sources[0]?.type ?? "Event";
    onChange([...outputs, newOutput(format, outputs.map((o) => o.name), type ?? "Event") as AnyOutput]);
  };
  const set = (index: number, next: AnyOutput): void => onChange(outputs.map((o, i) => (i === index ? next : o)));

  return (
    <section aria-labelledby={`${id}-heading`} className="flex flex-col gap-3">
      <h3 id={`${id}-heading`} className="text-body font-semibold">
        {t(`${k}.heading`)}
      </h3>
      {off ? (
        <p className="text-caption text-fg-muted">{off}</p>
      ) : (
        <>
          {outputs.length === 0 ? (
            <EmptyState bare icon="endpoints" title={t(`${k}.emptyTitle`)} description={t(`${k}.emptyHint`)} />
          ) : null}
          {twice.length > 0 ? <InlineError>{t("endpoints.outputs.duplicate", { names: twice.join(", ") })}</InlineError> : null}
          {outputs.map((output, index) => (
            <OutputCard
              key={index}
              id={`${id}-${index}`}
              format={format}
              output={output}
              duplicate={twice.includes(output.name)}
              onChange={(next) => set(index, next)}
              onRemove={() => onChange(outputs.filter((_, i) => i !== index))}
              renderSource={renderSource}
              frozen={frozen}
              endpoint={endpoint}
            />
          ))}
          {frozen ? null : (
            <div>
              <Button
                type="button"
                variant="secondary"
                onClick={add}
                disabled={full}
                disabledReason={full ? t("endpoints.outputs.full", { max: MAX_OUTPUTS }) : undefined}
              >
                {t(`${k}.add`)}
              </Button>
            </div>
          )}
        </>
      )}
    </section>
  );
}

interface OutputCardProps {
  id: string;
  format: Format;
  output: AnyOutput;
  duplicate: boolean;
  onChange: (next: AnyOutput) => void;
  onRemove: () => void;
  renderSource: OutputListProps["renderSource"];
  frozen: boolean;
  endpoint?: OutputListProps["endpoint"];
}

function OutputCard({ id, format, output, duplicate, onChange, onRemove, renderSource, frozen, endpoint }: OutputCardProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? "sk";
  const label = output.title?.[language] ?? Object.values(output.title ?? {})[0] ?? output.name;
  const nameErrors = [
    ...(isSlug(output.name) ? [] : [t("endpoints.outputs.nameField.bad")]),
    ...(duplicate ? [t("endpoints.outputs.duplicateOne")] : []),
  ];
  const address = format === "ical" ? `feeds/${output.name}.ics` : format === "feed" ? `feeds/${output.name}.atom` : "";
  const query = queryOf(output);
  const setSource = (index: number, next: AnySource): void =>
    onChange({ ...output, sources: output.sources.map((s, i) => (i === index ? next : s)) });

  return (
    <Card padding="md" className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-body font-semibold">{label}</h4>
        {frozen ? null : (
          <Button type="button" variant="ghost" size="sm" onClick={onRemove} aria-label={t("endpoints.outputs.removeNamed", { name: label })}>
            {t("endpoints.outputs.remove")}
          </Button>
        )}
      </div>
      <div className="grid gap-4 sm:grid-cols-3">
        <Field id={`${id}-name`} label={t("endpoints.outputs.nameField.label")} required help={format === "widget" ? t("endpoints.outputs.widgetName.hint") : t("endpoints.outputs.nameField.hint", { address })} errors={nameErrors}>
          <Input
            id={`${id}-name`}
            value={output.name}
            readOnly={frozen}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => onChange({ ...output, name: event.currentTarget.value.trim().toLowerCase() })}
          />
        </Field>
        {LANGUAGES.map((lang) => (
          <Field key={lang} id={`${id}-title-${lang}`} label={t(`endpoints.outputs.title.${lang}`)} help={t(`endpoints.outputs.${format}Title${lang === "sk" ? "Sk" : "En"}.hint`)}>
            <Input
              id={`${id}-title-${lang}`}
              value={output.title?.[lang] ?? ""}
              readOnly={frozen}
              onChange={(event) => {
                const title = withText(output.title, lang, event.currentTarget.value);
                const next = { ...output, title };
                if (!title) delete next.title;
                onChange(next);
              }}
            />
          </Field>
        ))}
      </div>
      {/* The widget's defaults narrow nothing: a reader's map shows what the table queries. */}
      {format === "widget" ? null : <Field id={`${id}-q`} label={t("endpoints.outputs.filterField.label")} help={t("endpoints.outputs.filterField.hint")}>
        <Textarea
          id={`${id}-q`}
          rows={3}
          placeholder='q=category=="sport"'
          value={query}
          readOnly={frozen}
          autoComplete="off"
          spellCheck={false}
          className="font-mono"
          onChange={(event) => onChange(withQuery(output, event.currentTarget.value))}
        />
      </Field>}
      {format === "widget" ? null : <PresetOptions id={id} format={format} output={output} onChange={onChange} frozen={frozen} />}
      {format === "ical" ? <IcalPreview id={`${id}-preview`} output={output} endpoint={endpoint} /> : null}
      {output.sources.map((source, index) => (
        <div key={index} className="flex flex-col gap-2 border-t border-border pt-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h5 className="text-caption font-semibold">{t("endpoints.outputs.source", { n: index + 1, type: source.type ?? "Event" })}</h5>
            {frozen || output.sources.length < 2 ? null : (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => onChange({ ...output, sources: output.sources.filter((_, i) => i !== index) })}
              >
                {t("endpoints.outputs.removeSource")}
              </Button>
            )}
          </div>
          {renderSource(`${id}-source-${index}`, source, (next) => setSource(index, next))}
        </div>
      ))}
      {frozen ? null : (
        <div>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            onClick={() =>
              onChange({ ...output, sources: [...output.sources, { ...(newOutput(format, [], "Event").sources[0] as AnySource) }] })
            }
          >
            {t("endpoints.outputs.sourceAdd.label")}
          </Button>
          <p className="mt-1 text-caption text-fg-muted">{t("endpoints.outputs.sourceAdd.hint")}</p>
        </div>
      )}
    </Card>
  );
}

interface PresetOptionsProps {
  id: string;
  format: Format;
  output: AnyOutput;
  onChange: (next: AnyOutput) => void;
  frozen: boolean;
}

/** A calendar's order, limit and zone; a feed's page size (API/02 §4a stored presets). */
function PresetOptions({ id, format, output, onChange, frozen }: PresetOptionsProps): JSX.Element {
  const { t } = useTranslation();
  const ical = format === "ical";
  const max = ical ? MAX_ICAL_LIMIT : MAX_FEED_LIMIT;
  const limit = output.limit;
  const sort = typeof output.sort === "string" ? output.sort : "";
  const zone = typeof output.timezone === "string" ? output.timezone : "";
  return (
    <div className="grid gap-4 sm:grid-cols-3">
      {ical ? (
        <Field id={`${id}-sort`} label={t("endpoints.outputs.sortField.label")} help={t("endpoints.outputs.sortField.hint")}>
          <Select id={`${id}-sort`} value={sort} disabled={frozen} onChange={(event) => onChange(withMember(output, "sort", event.currentTarget.value))}>
            <option value="">{t("endpoints.outputs.sortField.default")}</option>
            {ICAL_SORTS.map((s) => (
              <option key={s} value={s}>
                {t(`endpoints.outputs.sortField.${s === "start" ? "start" : "latest"}`)}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      <Field
        id={`${id}-limit`}
        label={t("endpoints.outputs.limitField.label")}
        help={t(ical ? "endpoints.outputs.limitField.hint" : "endpoints.outputs.limitField.feedHint")}
        errors={limitFits(limit, max) ? [] : [t("endpoints.outputs.limitField.bad", { max })]}
      >
        <Input
          id={`${id}-limit`}
          type="number"
          inputMode="numeric"
          min={1}
          max={max}
          step={1}
          placeholder={ical ? "100" : "50"}
          value={typeof limit === "number" ? String(limit) : ""}
          readOnly={frozen}
          onChange={(event) => {
            const text = event.currentTarget.value.trim();
            onChange(withMember(output, "limit", text === "" ? undefined : Number(text)));
          }}
        />
      </Field>
      {ical ? (
        <Field id={`${id}-timezone`} label={t("endpoints.outputs.timezoneField.label")} help={t("endpoints.outputs.timezoneField.hint")}>
          <Select
            id={`${id}-timezone`}
            value={zone || DEFAULT_ZONE}
            disabled={frozen}
            onChange={(event) => {
              const picked = event.currentTarget.value;
              onChange(withMember(output, "timezone", picked === DEFAULT_ZONE ? undefined : picked));
            }}
          >
            {timeZones(zone || undefined).map((z) => (
              <option key={z} value={z}>
                {z === DEFAULT_ZONE ? t("endpoints.outputs.timezoneField.default", { zone: z }) : z}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
    </div>
  );
}

interface IcalPreviewProps {
  id: string;
  output: AnyOutput;
  endpoint?: OutputListProps["endpoint"];
}

/** `20261115T190000` as `2026-11-15 19:00`; any other text as it is. */
function readableStart(start: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2}))?/.exec(start);
  if (!m) return start;
  return m[4] ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}` : `${m[1]}-${m[2]}-${m[3]}`;
}

/** The first entries the calendar gives, read through the saved Endpoint as the person (T-6762). */
function IcalPreview({ id, output, endpoint }: IcalPreviewProps): JSX.Element {
  const { t } = useTranslation();
  const preview = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/endpoints/{name}/ical-preview", {
          params: { path: endpoint ?? { project: "", name: "" } },
          body: { output: output as unknown as Record<string, unknown> },
        }),
      ),
  });
  const answer = preview.data;
  const skipped = answer
    ? Object.entries(answer.skipped)
        .map(([reason, n]) => t("endpoints.outputs.preview.reason", { n, reason: t(`endpoints.outputs.preview.reasons.${reason}`, { defaultValue: reason }) }))
        .join(", ")
    : "";
  const badLimit = !limitFits(output.limit, MAX_ICAL_LIMIT);
  const blocked = !endpoint ? t("endpoints.outputs.preview.unsaved") : badLimit ? t("endpoints.outputs.limitField.bad", { max: MAX_ICAL_LIMIT }) : undefined;
  return (
    <div className="flex flex-col gap-2" aria-live="polite">
      <div>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => preview.mutate()}
          disabled={Boolean(blocked) || preview.isPending}
          disabledReason={blocked}
        >
          {preview.isPending ? t("endpoints.outputs.preview.loading") : t("endpoints.outputs.preview.label")}
        </Button>
        <p id={`${id}-hint`} className="mt-1 text-caption text-fg-muted">
          {endpoint ? t("endpoints.outputs.preview.hint") : t("endpoints.outputs.preview.unsaved")}
        </p>
      </div>
      {preview.isError ? <InlineError>{reasonOf(preview.error, t("app.error"))}</InlineError> : null}
      {answer ? (
        <section data-testid={id} aria-labelledby={`${id}-heading`} className="flex flex-col gap-2 rounded-md border border-border bg-surface-subtle p-3">
          <h5 id={`${id}-heading`} className="text-caption font-semibold">
            {t("endpoints.outputs.preview.heading")}
          </h5>
          <p className="text-body text-fg">
            {t("endpoints.outputs.preview.counts", { read: answer.read, valid: answer.valid })}
            {answer.outside > 0 ? ` ${t("endpoints.outputs.preview.outside", { outside: answer.outside })}` : ""}
            {answer.events.length > 0 && answer.events.length < answer.valid
              ? ` ${t("endpoints.outputs.preview.shown", { shown: answer.events.length, valid: answer.valid })}`
              : ""}
          </p>
          {skipped ? <p className="text-body text-warning">{t("endpoints.outputs.preview.skipped", { reasons: skipped })}</p> : null}
          {answer.events.length === 0 ? (
            <p className="text-caption text-fg-muted">{t("endpoints.outputs.preview.none")}</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {entriesOf(answer.events).map((entry, index) => (
                <li key={index} className="flex flex-col break-words text-body">
                  <span className="font-mono text-caption text-fg-muted">
                    {readableStart(entry.start)}
                    {entry.zone ? ` ${entry.zone}` : ""}
                  </span>
                  <span>{entry.summary || t("endpoints.outputs.preview.untitled")}</span>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : null}
    </div>
  );
}

