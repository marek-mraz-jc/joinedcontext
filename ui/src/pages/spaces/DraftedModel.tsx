/**
 * The data model a file will make, drawn before anything is proposed (T-6567): the class and each
 * attribute with its type in plain words, the way the Models page draws a model.
 */
import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { parseDocument } from "yaml";
import { LinkmlGraphView } from "../models/LinkmlGraphView";
import { importSlotsOf, LATITUDE, LONGITUDE, suggestMapping } from "./importRows";
import type { Table } from "./importRows";

const WORDS = new Set(["string", "integer", "float", "boolean", "date", "datetime", "GeoProperty"]);

/**
 * `source` with the position its rows will carry drawn as one attribute: an entity's `location`
 * comes from Entity and is in no drafted model, yet a latitude and a longitude column fill it.
 * Only the picture reads this text; the model proposed stays the drafted one.
 */
export function withPlace(source: string, type: string, table: Table): string {
  const slots = importSlotsOf(source, type);
  const place = slots.find((slot) => slot.kind === "GeoProperty");
  const mapped = Object.values(suggestMapping(table.columns, slots));
  if (!place || !mapped.includes(LATITUDE) || !mapped.includes(LONGITUDE)) return source;
  const doc = parseDocument(source);
  if (doc.getIn(["slots", place.name]) !== undefined || doc.getIn(["classes", type, "slots"]) === undefined) return source;
  doc.addIn(["classes", type, "slots"], place.name);
  doc.setIn(["slots", place.name], doc.createNode({ range: "GeoProperty" }));
  return doc.toString();
}

export function DraftedModel({ source, type, table }: { source: string | undefined; type: string; table: Table }): JSX.Element | null {
  const { t } = useTranslation();
  const shown = useMemo(() => (source === undefined ? undefined : withPlace(source, type, table)), [source, type, table]);
  if (shown === undefined) return null;
  const typeWord = (range: string) => (WORDS.has(range) ? t(`spaces.complete.review.range.${range}`) : range);
  return (
    <section aria-labelledby="drafted-model" data-testid="drafted-model" className="flex min-w-0 flex-col gap-2">
      <h2 id="drafted-model" className="text-body font-semibold text-fg">
        {t("spaces.complete.diagram.title", { type })}
      </h2>
      <p className="text-caption text-fg-muted">{t("spaces.complete.diagram.hint")}</p>
      <LinkmlGraphView source={shown} typeWord={typeWord} />
    </section>
  );
}
