/**
 * The drafted model, read before it is proposed: each attribute with its type and a value from the
 * file, and the type open to correction (T-6456). A correction goes back to the op as the model
 * file it is, so the op checks it the way it checks any model.
 */
import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { parseDocument } from "yaml";
import { useOrgDomain } from "../../api/projects";
import { Select } from "../../components/ui";
import { importSlotsOf, rowIds, suggestMapping, toEntities } from "./importRows";
import type { ImportSlot, Mapping, Table } from "./importRows";
import { RowsCheck } from "./LoadRows";

/** The types a column of a table can take; a slot naming another range keeps it and shows it read only. */
const RANGES = ["string", "integer", "float", "boolean", "date", "datetime"];

/** `source` with `slot` of range `range`, wherever the model declares it; the rest of the text stays. */
export function withRange(source: string, type: string, slot: string, range: string): string {
  const doc = parseDocument(source);
  const at = doc.hasIn(["slots", slot]) ? ["slots", slot] : ["classes", type, "attributes", slot];
  doc.setIn([...at, "range"], range);
  return doc.toString();
}

/**
 * The first value of the slot's column, so a person judges its type by what the file holds. A
 * column mapped elsewhere (latitude and longitude into a position) is still the slot's by title.
 */
function sampleOf(table: Table, mapping: Mapping, slot: ImportSlot): string {
  const column = Object.keys(mapping).find((name) => mapping[name] === slot.name) ?? table.columns.find((name) => slot.titles?.includes(name));
  if (column === undefined) return "";
  return table.rows.find((row) => (row[column] ?? "") !== "")?.[column] ?? "";
}

export function ModelReview({ project, space, type, source, table, onCorrect }: {
  project: string;
  space: string;
  type: string;
  source: string | undefined;
  table: Table;
  onCorrect: (source: string) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const orgDomain = useOrgDomain(project);
  const every = useMemo(() => importSlotsOf(source, type), [source, type]);
  const mapping = useMemo(() => suggestMapping(table.columns, every), [table, every]);
  // The position every entity may carry is no attribute of a table with no such column.
  const slots = every.filter((slot) => slot.kind !== "GeoProperty" || Object.values(mapping).includes(slot.name));
  const checked = useMemo(() => toEntities(table, mapping, every, { type, orgDomain, space }, rowIds(table)), [table, mapping, every, type, orgDomain, space]);
  if (source === undefined || slots.length === 0) return null;
  return (
    <section aria-labelledby="model-review" data-testid="model-review" className="flex flex-col gap-2">
      <h2 id="model-review" className="text-body font-semibold text-fg">
        {t("spaces.complete.review.title", { type, count: slots.length })}
      </h2>
      <p className="text-caption text-fg-muted">{t("spaces.complete.review.hint")}</p>
      <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
        {slots.map((slot) => {
          const label = slot.titles?.[0] ?? slot.name;
          const sample = sampleOf(table, mapping, slot);
          const id = `review-${slot.name}`;
          return (
            <li key={slot.name} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 px-3 py-2">
              <span className="flex min-w-0 flex-col">
                <label htmlFor={id} className="text-body text-fg">
                  {label}
                </label>
                <span className="truncate text-caption text-fg-muted">{sample === "" ? t("spaces.complete.review.noSample") : t("spaces.complete.review.sample", { value: sample })}</span>
              </span>
              {slot.range !== undefined && RANGES.includes(slot.range) ? (
                <Select id={id} className="w-32" value={slot.range} onChange={(e) => onCorrect(withRange(source, type, slot.name, e.target.value))}>
                  {RANGES.map((range) => (
                    <option key={range} value={range}>
                      {t(`spaces.complete.review.range.${range}`)}
                    </option>
                  ))}
                </Select>
              ) : (
                <span className="font-mono text-caption text-fg-muted">{slot.range ?? slot.kind}</span>
              )}
            </li>
          );
        })}
      </ul>
      <RowsCheck checked={checked} />
    </section>
  );
}
