/**
 * The mapper's editors past a plain field (G-18): a Relationship's key field, one field per
 * language, a geometry field with its coordinate system, joined fields, an attribute's
 * `observedAt` and the value transforms. Every choice is a menu; what a person types (a default)
 * is written as a string literal by `outputMapper.ts`, never as code.
 */
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Button, Field, Input, Select } from "../../components/ui";
import { CRS_CODES, DATE_FORMATS, shapedValue } from "./outputMapper";
import type { Attribute, Source, Transform } from "./outputMapper";

/** The language tags offered for a LanguageProperty, the Portal's own and the cities' languages. */
export const LANGUAGE_TAGS = ["en", "sk", "cs", "de", "ca", "es", "fi", "sv", "fr", "it", "pl", "hu", "nl", "pt"] as const;

/** The separators offered for joined fields. */
const SEPARATORS = [" ", ", ", " - ", " / ", ""] as const;

/** Transforms that take no parameter, in the order the menu offers them. */
const PLAIN = ["trim", "lower", "upper", "decimalComma"] as const;

/** The most distinct sample values a lookup table lists. */
const LOOKUP_ROWS = 20;

interface FieldPickProps {
  id: string;
  label: string;
  hint: string;
  value: string;
  fields: string[];
  onChange: (field: string) => void;
  /** Offer "not mapped" first; the field is then optional. */
  optional?: boolean;
}

/** A record field picked from the sample, keeping one the sample lacks so an edit never loses it. */
export function FieldPick({ id, label, hint, value, fields, onChange, optional }: FieldPickProps): JSX.Element {
  const { t } = useTranslation();
  const known = value === "" || fields.includes(value) ? fields : [...fields, value];
  return (
    <Field id={id} label={label} description={hint}>
      <Select id={id} value={value} onChange={(event) => onChange(event.target.value)}>
        {optional || value === "" ? <option value="">{t("pipelines.mapper.notMapped")}</option> : null}
        {known.map((field) => (
          <option key={field} value={field}>
            {field}
          </option>
        ))}
      </Select>
    </Field>
  );
}

interface EditorProps {
  id: string;
  attribute: Attribute;
  source: Source;
  fields: string[];
  onChange: (source: Source) => void;
  /** The sample's records, whose values a lookup table lists. */
  records?: Record<string, unknown>[];
}

/** The editor of a source that is not a field, an expression or a fixed value; none for those. */
export function SourceEditor({ id, attribute, source, fields, onChange, records }: EditorProps): JSX.Element | null {
  const { t } = useTranslation();
  const m = (key: string, options?: Record<string, unknown>) => t(`pipelines.mapper.${key}`, options ?? {});
  if ("ref" in source) {
    return (
      <FieldPick
        id={`${id}-ref`}
        label={m("refField", { target: source.ref.type })}
        hint={m("hint.refField", { target: source.ref.type })}
        value={source.ref.field}
        fields={fields}
        onChange={(field) => onChange({ ref: { ...source.ref, field } })}
      />
    );
  }
  if ("languages" in source) {
    const rows = Object.entries(source.languages);
    const free = LANGUAGE_TAGS.filter((tag) => !(tag in source.languages));
    const set = (next: [string, string][]) => onChange({ languages: Object.fromEntries(next) });
    return (
      <fieldset className="flex flex-col gap-2" aria-describedby={`${id}-languages-hint`}>
        <legend className="text-caption font-medium text-fg">{m("languages")}</legend>
        <p id={`${id}-languages-hint`} className="text-caption text-fg-muted">
          {m("hint.languages")}
        </p>
        {rows.map(([tag, field], index) => (
          <div key={tag} className="grid items-end gap-2 sm:grid-cols-[8rem_1fr_auto]">
            <Field id={`${id}-lang-${index}`} label={m("language")} description={m("hint.language")}>
              <Select
                id={`${id}-lang-${index}`}
                value={tag}
                onChange={(event) => set(rows.map((row, at) => (at === index ? [event.target.value, row[1]] : row)))}
              >
                {[tag, ...free].map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </Select>
            </Field>
            <FieldPick
              id={`${id}-lang-${index}-field`}
              label={m("languageField", { tag })}
              hint={m("hint.languageField", { tag })}
              value={field}
              fields={fields}
              onChange={(next) => set(rows.map((row, at) => (at === index ? [row[0], next] : row)))}
            />
            <Button size="sm" variant="ghost" onClick={() => set(rows.filter((_, at) => at !== index))} aria-label={m("languageRemove", { tag })}>
              {m("remove")}
            </Button>
          </div>
        ))}
        {free.length > 0 ? (
          <Button size="sm" variant="secondary" className="self-start" onClick={() => set([...rows, [free[0], fields[0] ?? ""]])}>
            {m("languageAdd")}
          </Button>
        ) : null}
      </fieldset>
    );
  }
  if ("geometry" in source || "longitude" in source) {
    return (
      <div className="grid gap-2 sm:grid-cols-2">
        {"geometry" in source ? (
          <FieldPick
            id={`${id}-geometry`}
            label={m(source.format === "wkt" ? "wktField" : "geojsonField")}
            hint={m(source.format === "wkt" ? "hint.wktField" : "hint.geojsonField")}
            value={source.geometry}
            fields={fields}
            onChange={(geometry) => onChange({ ...source, geometry })}
          />
        ) : null}
        <Field id={`${id}-crs`} label={m("crs")} description={m("hint.crs")}>
          <Select id={`${id}-crs`} value={source.crs ?? "EPSG:4326"} onChange={(event) => onChange({ ...source, crs: event.target.value === "EPSG:4326" ? undefined : event.target.value })}>
            {CRS_CODES.map((code) => (
              <option key={code} value={code}>
                {t(`pipelines.mapper.crsName.${code.replace(":", "_")}`, { defaultValue: code })}
              </option>
            ))}
          </Select>
        </Field>
      </div>
    );
  }
  if ("concat" in source) {
    const set = (concat: string[]) => onChange({ ...source, concat });
    return (
      <fieldset className="flex flex-col gap-2" aria-describedby={`${id}-concat-hint`}>
        <legend className="text-caption font-medium text-fg">{m("concat")}</legend>
        <p id={`${id}-concat-hint`} className="text-caption text-fg-muted">
          {m("hint.concat")}
        </p>
        {source.concat.map((field, index) => (
          <div key={index} className="grid items-end gap-2 sm:grid-cols-[1fr_auto]">
            <FieldPick
              id={`${id}-concat-${index}`}
              label={m("concatPart", { number: index + 1 })}
              hint={m("hint.concatPart")}
              value={field}
              fields={fields}
              onChange={(next) => set(source.concat.map((one, at) => (at === index ? next : one)))}
            />
            <Button size="sm" variant="ghost" onClick={() => set(source.concat.filter((_, at) => at !== index))} aria-label={m("concatRemove", { field })}>
              {m("remove")}
            </Button>
          </div>
        ))}
        <Button size="sm" variant="secondary" className="self-start" onClick={() => set([...source.concat, fields[0] ?? ""])}>
          {m("concatAdd")}
        </Button>
        <Field id={`${id}-separator`} label={m("separator")} description={m("hint.separator")}>
          <Select id={`${id}-separator`} value={source.separator} onChange={(event) => onChange({ ...source, separator: event.target.value })}>
            {SEPARATORS.map((separator) => (
              <option key={separator} value={separator}>
                {separator === "" ? m("separatorNone") : separator === " " ? m("separatorSpace") : `“${separator}”`}
              </option>
            ))}
          </Select>
        </Field>
      </fieldset>
    );
  }
  if ("field" in source) return <TransformsEditor id={id} attribute={attribute} source={source} fields={fields} onChange={onChange} records={records} />;
  return null;
}

/** The distinct values a field holds in the sample, as text, up to `LOOKUP_ROWS`; a lookup lists them as the steps before it leave them. */
export function sampleValues(records: Record<string, unknown>[], field: string): string[] {
  const seen = new Set<string>();
  for (const record of records) {
    const value = record[field];
    if (value === undefined || value === null || value === "") continue;
    seen.add(typeof value === "string" ? value : JSON.stringify(value));
    if (seen.size >= LOOKUP_ROWS) break;
  }
  return [...seen];
}

interface TransformsProps extends EditorProps {
  source: Extract<Source, { field: string }>;
}

/** The transforms of a field source: a list with one menu to add more (G-18). */
export function TransformsEditor({ id, attribute, source, onChange, records = [] }: TransformsProps): JSX.Element {
  const { t } = useTranslation();
  const m = (key: string, options?: Record<string, unknown>) => t(`pipelines.mapper.${key}`, options ?? {});
  const transforms = source.transforms ?? [];
  const set = (next: Transform[]) => onChange(next.length > 0 ? { field: source.field, transforms: next } : { field: source.field });
  const has = (op: Transform["op"]) => transforms.some((transform) => transform.op === op);
  const offered = [
    ...PLAIN.filter((op) => !has(op)),
    ...(has("date") ? [] : ["date"]),
    ...(attribute.values && !has("lookup") ? ["lookup"] : []),
    ...(has("default") ? [] : ["default"]),
  ];
  const add = (op: string) => {
    const made: Transform =
      op === "date"
        ? { op: "date", format: DATE_FORMATS[0] }
        : op === "lookup"
          ? { op: "lookup", table: {} }
          : op === "default"
            ? { op: "default", value: attribute.values?.[0] ?? "" }
            : { op: op as (typeof PLAIN)[number] };
    // The default stays last: it is what an empty field becomes, after every other step.
    const rest = transforms.filter((transform) => transform.op !== "default");
    const fallback = transforms.find((transform) => transform.op === "default");
    set(made.op === "default" ? [...rest, made] : [...rest, made, ...(fallback ? [fallback] : [])]);
  };
  const replace = (index: number, transform: Transform) => set(transforms.map((one, at) => (at === index ? transform : one)));
  return (
    <div className="flex flex-col gap-2" data-testid={`${id}-transforms`}>
      {offered.length > 0 ? (
        <Field id={`${id}-add-transform`} label={m("transformAdd")} description={m("hint.transformAdd")}>
          <Select
            id={`${id}-add-transform`}
            value=""
            onChange={(event) => {
              if (event.target.value) add(event.target.value);
            }}
          >
            <option value="">{m("transformPick")}</option>
            {offered.map((op) => (
              <option key={op} value={op}>
                {m(`transform.${op}`)}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      {transforms.length > 0 ? (
        <ol className="flex flex-col gap-2" aria-label={m("transforms", { name: attribute.name })}>
          {transforms.map((transform, index) => (
            <li key={`${transform.op}-${index}`} className="flex flex-col gap-1 rounded border border-border bg-surface-subtle p-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-caption font-medium text-fg">{m(`transform.${transform.op}`)}</span>
                <Button size="sm" variant="ghost" onClick={() => set(transforms.filter((_, at) => at !== index))} aria-label={m("transformRemove", { what: m(`transform.${transform.op}`) })}>
                  {m("remove")}
                </Button>
              </div>
              {transform.op === "date" ? (
                <Field id={`${id}-date-${index}`} label={m("dateFormat")} description={m("hint.dateFormat")}>
                  <Select id={`${id}-date-${index}`} value={transform.format} onChange={(event) => replace(index, { op: "date", format: event.target.value })}>
                    {DATE_FORMATS.map((format) => (
                      <option key={format} value={format}>
                        {m(`dateFormats.${DATE_FORMATS.indexOf(format)}`)}
                      </option>
                    ))}
                  </Select>
                </Field>
              ) : null}
              {transform.op === "lookup" && attribute.values ? (
                <LookupTable
                  id={`${id}-lookup`}
                  values={attribute.values}
                  seen={[...new Set(sampleValues(records, source.field).map((value) => String(shapedValue(value, transforms.slice(0, index), attribute) ?? value)))]}
                  table={transform.table}
                  onChange={(table) => replace(index, { op: "lookup", table })}
                />
              ) : null}
              {transform.op === "default" ? (
                <Field id={`${id}-default-${index}`} label={t("pipelines.mapper.defaultValue")} description={t("pipelines.mapper.hint.defaultValue")}>
                  {attribute.values ? (
                    <Select id={`${id}-default-${index}`} value={transform.value} onChange={(event) => replace(index, { op: "default", value: event.target.value })}>
                      {attribute.values.map((value) => (
                        <option key={value} value={value}>
                          {value}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      id={`${id}-default-${index}`}
                      type={attribute.valueType === "number" || attribute.valueType === "integer" ? "number" : "text"}
                      value={transform.value}
                      onChange={(event) => replace(index, { op: "default", value: event.target.value })}
                    />
                  )}
                </Field>
              ) : (
                <p className="text-caption text-fg-muted">{m(`transformHint.${transform.op}`)}</p>
              )}
            </li>
          ))}
        </ol>
      ) : null}
    </div>
  );
}

interface LookupProps {
  id: string;
  values: string[];
  /** The values the sample holds, each a row. */
  seen: string[];
  table: Record<string, string>;
  onChange: (table: Record<string, string>) => void;
}

/** Source value to model value, one row per value the sample holds, a value the model has already left out. */
function LookupTable({ id, values, seen, table, onChange }: LookupProps): JSX.Element {
  const { t } = useTranslation();
  const rows = [...new Set([...Object.keys(table), ...seen.filter((value) => !values.includes(value))])];
  if (rows.length === 0) return <p className="text-caption text-fg-muted">{t("pipelines.mapper.lookupEmpty")}</p>;
  return (
    <fieldset className="flex flex-col gap-1">
      <legend className="text-caption text-fg-muted">{t("pipelines.mapper.lookupHint")}</legend>
      {rows.map((from, index) => (
        <Field key={from} id={`${id}-${index}`} label={t("pipelines.mapper.lookupRow", { from })} description={t("pipelines.mapper.hint.lookupRow", { example: values[0] })}>
          <Select
            id={`${id}-${index}`}
            value={table[from] ?? ""}
            onChange={(event) => {
              const next = { ...table };
              if (event.target.value) next[from] = event.target.value;
              else delete next[from];
              onChange(next);
            }}
          >
            <option value="">{t("pipelines.mapper.lookupKeep")}</option>
            {values.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </Field>
      ))}
    </fieldset>
  );
}
