/**
 * The editor's Lifetime step (T-4842, PL-80, PL-64, TS 104 175 4.22): when the entities of the
 * type a pipeline writes, or some of their attributes, leave the space. Keep them; end the whole
 * entity from a field (plus a while, with a cap), a while after each write, or on a fixed date;
 * end only chosen attributes; or let the sweep delete those the source stopped sending. The
 * choice is the pipeline's `spec.lifetime` (or `spec.expiry`), never Bloblang, and a sentence
 * under it says exactly what happens.
 */
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { RecordLink } from "../../components/RecordLink";
import { RetentionGuide } from "../../components/RetentionGuide";
import { Button, Field, Input, RadioGroup, Select } from "../../components/ui";
import { fromIso, toIso, UNITS } from "../spaces/retention";
import type { Unit } from "../spaces/retention";
import {
  asKind,
  choiceOf,
  endKindOf,
  entriesOf,
  fromLocalInput,
  OBSERVED_AT,
  problemOf,
  rowsOf,
  sentence,
  toLocalInput,
} from "./lifetime";
import type { AttributeRow, Choice, EndKind, Expiry, Lifetime, LifetimeMode } from "./lifetime";

/** The caps a `from` end offers, besides the one the manifest holds. */
const CAPS = ["PT1H", "P1D", "P7D", "P30D"];

export interface LifetimeChoiceProps {
  project: string;
  space: string;
  type: string;
  /** The type's date-time attributes, the times an end can count from. */
  timeSlots: string[];
  /** The attributes the pipeline may write through its Endpoint: the only ones an attribute end names. */
  attributes: string[];
  entry: Lifetime | undefined;
  expiry: Expiry | undefined;
  /** Sets the type's entry and the sweep together, so one never overwrites the other. */
  onChange: (entry: Omit<Lifetime, "type"> | undefined, expiry: Expiry | undefined) => void;
  /** Whether the form holds `spec.expiry`: the sweep is offered only then. */
  sweep: boolean;
  now?: Date;
}

function SpanInput({ id, label, value, onChange }: { id: string; label: string; value: string | undefined; onChange: (iso: string) => void }): JSX.Element {
  const { t } = useTranslation();
  const keep = value ? fromIso(value) : undefined;
  const set = (amount: string, unit: Unit) => {
    const iso = toIso({ amount: Number(amount), unit });
    if (iso) onChange(iso);
  };
  return (
    <Field id={id} label={label}>
      <span className="flex gap-2">
        <Input id={id} type="number" min={1} className="w-20" value={keep ? String(keep.amount) : ""} onChange={(event) => set(event.target.value, keep?.unit ?? "minutes")} />
        {/* The unit keeps its words whole beside the amount, at any width. */}
        <span className="min-w-32 shrink-0">
          <Select aria-label={t("pipelines.lifetime.unitOf", { what: label })} value={keep?.unit ?? "minutes"} onChange={(event) => set(String(keep?.amount ?? 1), event.target.value as Unit)}>
            {UNITS.map((unit) => (
              <option key={unit} value={unit}>
                {t(`spaces.retention.unit.${unit}`)}
              </option>
            ))}
          </Select>
        </span>
      </span>
    </Field>
  );
}

interface EndFieldsProps {
  id: string;
  mode: LifetimeMode;
  timeSlots: string[];
  onMode: (mode: LifetimeMode) => void;
  now: Date;
}

/** One end's fields: what it counts from and how long, or the date. */
function EndFields({ id, mode, timeSlots, onMode, now }: EndFieldsProps): JSX.Element {
  const { t } = useTranslation();
  const kind = endKindOf(mode);
  const problem = problemOf(mode, t, now);
  const caps = [...new Set([...CAPS, ...(mode.notAfter ? [mode.notAfter] : [])])];
  return (
    <div className="flex flex-wrap items-end gap-3">
      {kind === "from" ? (
        <>
          <Field id={`${id}-from`} label={t("pipelines.lifetime.from")} description={timeSlots.length === 0 ? t("pipelines.lifetime.noTimeSlot") : undefined}>
            <Select id={`${id}-from`} value={mode.from ?? OBSERVED_AT} onChange={(event) => onMode({ ...mode, from: event.target.value })}>
              <option value={OBSERVED_AT}>{t("pipelines.lifetime.observedAt")}</option>
              {[...new Set([...timeSlots, ...(mode.from && mode.from !== OBSERVED_AT ? [mode.from] : [])])].map((slot) => (
                <option key={slot} value={slot}>
                  {slot}
                </option>
              ))}
            </Select>
          </Field>
          <SpanInput id={`${id}-plus`} label={t("pipelines.lifetime.plus")} value={mode.plus} onChange={(plus) => onMode({ ...mode, plus })} />
          <Field id={`${id}-cap`} label={t("pipelines.lifetime.cap")}>
            <Select
              id={`${id}-cap`}
              value={mode.notAfter ?? ""}
              onChange={(event) => {
                onMode({ ...mode, notAfter: event.target.value || undefined });
              }}
            >
              <option value="">{t("pipelines.lifetime.noCap")}</option>
              {caps.map((cap) => (
                <option key={cap} value={cap}>
                  {t("pipelines.lifetime.capOf", { span: fromIso(cap) ? `${fromIso(cap)?.amount} ${t(`spaces.retention.unit.${fromIso(cap)?.unit ?? "days"}`)}` : cap })}
                </option>
              ))}
            </Select>
          </Field>
        </>
      ) : kind === "rolling" ? (
        <SpanInput id={`${id}-rolling`} label={t("pipelines.lifetime.keepFor")} value={mode.rolling} onChange={(rolling) => onMode({ rolling })} />
      ) : (
        <Field id={`${id}-at`} label={t("pipelines.lifetime.at")} description={t("pipelines.lifetime.atHint")} errors={problem ? [problem] : undefined}>
          <Input id={`${id}-at`} type="datetime-local" value={toLocalInput(mode.at)} onChange={(event) => onMode({ at: fromLocalInput(event.target.value) ?? "" })} />
        </Field>
      )}
    </div>
  );
}

export function LifetimeChoice({ project, space, type, timeSlots, attributes, entry, expiry, onChange, sweep, now = new Date() }: LifetimeChoiceProps): JSX.Element {
  const { t } = useTranslation();
  const choice = choiceOf(entry, expiry, type);
  const rows = rowsOf(entry);
  // A row added names the first granted field without an end yet: a row without one is no entry.
  const unused = attributes.find((name) => !rows.some((row) => row.name === name));
  const withoutType = (sweepOf: Expiry | undefined): Expiry | undefined => {
    const types = (sweepOf?.types ?? []).filter((one) => one !== type);
    return types.length > 0 ? { ...sweepOf, types } : undefined;
  };
  const choose = (next: Choice) => {
    const rest = withoutType(expiry);
    if (next === "keep") onChange(undefined, rest);
    if (next === "entity") onChange({ entity: entry?.entity ?? { from: OBSERVED_AT, plus: "PT10M" } }, rest);
    if (next === "attributes") onChange({ attributes: entry?.attributes?.length ? entry.attributes : [{ names: [attributes[0] ?? ""].filter(Boolean), from: OBSERVED_AT, plus: "PT10M" }] }, rest);
    if (next === "stale") onChange(undefined, { after: expiry?.after ?? "14d", types: [...new Set([...(expiry?.types ?? []), type])] });
  };
  const setRows = (next: AttributeRow[]) => onChange({ ...(entry?.entity ? { entity: entry.entity } : {}), attributes: entriesOf(next) }, expiry);
  const setRow = (index: number, row: AttributeRow) => setRows(rows.map((one, at) => (at === index ? row : one)));
  const kindOptions: { value: EndKind; label: string; description: string }[] = [
    { value: "from", label: t("pipelines.lifetime.kind.from"), description: t("pipelines.lifetime.kind.fromHint") },
    { value: "rolling", label: t("pipelines.lifetime.kind.rolling"), description: t("pipelines.lifetime.kind.rollingHint") },
    { value: "at", label: t("pipelines.lifetime.kind.at"), description: t("pipelines.lifetime.kind.atHint") },
  ];
  const after = /^([1-9][0-9]*)([hd])$/.exec(expiry?.after ?? "");

  return (
    <div className="flex flex-col gap-3" data-testid="pipeline-lifetime">
      <RadioGroup
        name="pipeline-lifetime-choice"
        legend={t("pipelines.lifetime.legend", { type })}
        value={choice}
        onChange={choose}
        options={[
          { value: "keep" as const, label: t("pipelines.lifetime.keep"), description: t("pipelines.lifetime.keepHint") },
          { value: "entity" as const, label: t("pipelines.lifetime.entity"), description: t("pipelines.lifetime.entityHint", { type }) },
          { value: "attributes" as const, label: t("pipelines.lifetime.attributes"), description: t("pipelines.lifetime.attributesHint", { type }) },
          ...(sweep ? [{ value: "stale" as const, label: t("pipelines.lifetime.stale"), description: t("pipelines.lifetime.staleHint") }] : []),
        ]}
      />

      {entry?.entity ? (
        <section aria-label={t("pipelines.lifetime.entity")} className="flex flex-col gap-2 rounded border border-border p-2">
          <RadioGroup
            name="pipeline-lifetime-entity-kind"
            legend={t("pipelines.lifetime.kind.legend")}
            layout="row"
            value={endKindOf(entry.entity)}
            options={kindOptions}
            onChange={(kind) => onChange({ ...entry, entity: asKind(entry.entity ?? {}, kind, now) }, expiry)}
          />
          <EndFields id="pipeline-lifetime-entity" mode={entry.entity} timeSlots={timeSlots} now={now} onMode={(entity) => onChange({ ...entry, entity }, expiry)} />
        </section>
      ) : null}

      {choice === "attributes" || (entry?.entity && rows.length > 0) ? (
        <section aria-label={t("pipelines.lifetime.attributes")} className="flex flex-col gap-2">
          {attributes.length === 0 ? <p className="text-caption text-fg-subtle">{t("pipelines.lifetime.noAttributes", { type })}</p> : null}
          {rows.map((row, index) => (
            <div key={index} className="flex flex-col gap-2 rounded border border-border p-2">
              <div className="flex flex-wrap items-end gap-3">
                <Field id={`pipeline-lifetime-row-${index}-name`} label={t("pipelines.lifetime.attribute")}>
                  <Select id={`pipeline-lifetime-row-${index}-name`} value={row.name} onChange={(event) => setRow(index, { ...row, name: event.target.value })}>
                    <option value="">{t("form.choose")}</option>
                    {[...new Set([...attributes, ...(row.name ? [row.name] : [])])].map((name) => (
                      <option key={name} value={name}>
                        {name}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field id={`pipeline-lifetime-row-${index}-kind`} label={t("pipelines.lifetime.kind.legend")}>
                  <Select id={`pipeline-lifetime-row-${index}-kind`} value={endKindOf(row)} onChange={(event) => setRow(index, { name: row.name, ...asKind(row, event.target.value as EndKind, now) })}>
                    {kindOptions.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Button size="xs" variant="secondary" onClick={() => setRows(rows.filter((_, at) => at !== index))} aria-label={t("pipelines.lifetime.removeOf", { name: row.name || index + 1 })}>
                  {t("pipelines.lifetime.remove")}
                </Button>
              </div>
              <EndFields id={`pipeline-lifetime-row-${index}`} mode={row} timeSlots={timeSlots} now={now} onMode={(mode) => setRow(index, { name: row.name, ...mode })} />
            </div>
          ))}
          <span>
            <Button
              size="xs"
              variant="secondary"
              disabled={unused === undefined}
              disabledReason={attributes.length === 0 ? t("pipelines.lifetime.noAttributes", { type }) : t("pipelines.lifetime.allAdded")}
              onClick={() => unused && setRows([...rows, { name: unused, from: OBSERVED_AT, plus: "PT10M" }])}
            >
              {t("pipelines.lifetime.add")}
            </Button>
          </span>
        </section>
      ) : null}

      {choice === "stale" ? (
        <Field id="pipeline-lifetime-after" label={t("pipelines.lifetime.after")} description={t("pipelines.lifetime.afterHint", { type })}>
          <span className="flex gap-2">
            <Input id="pipeline-lifetime-after" type="number" min={1} className="w-20" value={after?.[1] ?? ""} onChange={(event) => onChange(undefined, { ...expiry, after: `${event.target.value}${after?.[2] ?? "d"}` })} />
            <Select aria-label={t("pipelines.lifetime.afterUnit")} value={after?.[2] ?? "d"} onChange={(event) => onChange(undefined, { ...expiry, after: `${after?.[1] ?? "1"}${event.target.value}` })}>
              <option value="h">{t("spaces.retention.unit.hours")}</option>
              <option value="d">{t("spaces.retention.unit.days")}</option>
            </Select>
          </span>
        </Field>
      ) : null}

      <p role="status" className="rounded bg-surface-muted p-2 text-body text-fg" data-testid="pipeline-lifetime-sentence">
        {sentence(type, entry, expiry, t)}
      </p>
      <p className="text-caption text-fg-muted">
        {t("pipelines.lifetime.history", { type })}{" "}
        <RecordLink project={project} plural="spaces" name={space}>
          {t("pipelines.lifetime.spaceRules", { space })}
        </RecordLink>{" "}
        <RetentionGuide section="4-recipe-buses-and-live-vehicles" />
      </p>
    </div>
  );
}
