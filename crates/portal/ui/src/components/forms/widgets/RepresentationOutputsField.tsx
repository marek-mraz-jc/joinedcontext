import { useContext } from "react";
import type { JSX } from "react";
import type { FieldProps } from "@rjsf/utils";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../../api/client";
import { asManifests, refName } from "../../../api/manifest";
import { Button, Card, EmptyState, Field, InlineError, Input } from "../../ui";
import { LIST_BODY, LIST_FIELDSET, ListLegend } from "../theme";
import { FormDataContext } from "./EntitySelectorField";
import { ListFailed } from "./ListFailed";
import { FormProjectContext } from "./ModelWidgets";
import { FeedSourceEditor, IcalSourceEditor, MapSourceEditor } from "./OutputSourceEditors";
import { duplicateNames, isSlug, MAX_OUTPUTS, newOutput, outputsOf, withText, written } from "./representationOutputs";
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
  const root = useContext(FormDataContext) as { contextSpaceRef?: unknown; enabledRepresentations?: unknown } | undefined;
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
}

/** One format's outputs: a card per output, an add button, and what to do when there is none. */
function OutputList({ id, format, outputs, off, onChange, renderSource, frozen }: OutputListProps): JSX.Element {
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
}

function OutputCard({ id, format, output, duplicate, onChange, onRemove, renderSource, frozen }: OutputCardProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? "sk";
  const label = output.title?.[language] ?? Object.values(output.title ?? {})[0] ?? output.name;
  const nameErrors = [
    ...(isSlug(output.name) ? [] : [t("endpoints.outputs.nameField.bad")]),
    ...(duplicate ? [t("endpoints.outputs.duplicateOne")] : []),
  ];
  const address = format === "ical" ? `ical/v1/${output.name}.ics` : format === "feed" ? `feed/v1/${output.name}/atom.xml` : "";
  const q = output.filter?.q ?? "";
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
        <Input
          id={`${id}-q`}
          value={q}
          readOnly={frozen}
          autoComplete="off"
          spellCheck={false}
          className="font-mono"
          onChange={(event) => {
            const value = event.currentTarget.value;
            const filter = { ...(output.filter ?? {}), q: value || undefined };
            if (!filter.q) delete filter.q;
            const { filter: _old, ...rest } = output;
            onChange(Object.keys(filter).length === 0 ? rest : { ...rest, filter });
          }}
        />
      </Field>}
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

