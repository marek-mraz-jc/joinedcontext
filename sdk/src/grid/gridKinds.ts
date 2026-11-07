import type { ColumnKind } from "./cells";
import type { GridLabels } from "./useEntityGrid";

/** The glyph a header shows for what its column holds; the kind's name is what a screen reader hears. */
export const KIND_ICON: Record<ColumnKind, string> = {
  text: "Aa",
  number: "#",
  boolean: "✓",
  datetime: "◷",
  geo: "⌖",
  relationship: "↗",
  language: "文",
  structured: "{}",
  list: "≡",
};

export const KIND_LABEL: Record<ColumnKind, keyof GridLabels> = {
  text: "kindText",
  number: "kindNumber",
  boolean: "kindBoolean",
  datetime: "kindDatetime",
  geo: "kindGeo",
  relationship: "kindRelationship",
  language: "kindLanguage",
  structured: "kindStructured",
  list: "kindList",
};
