/**
 * The columns of a type as the model orders and names them (DM-98, DM-100), computed on what the
 * caller's own answer carries (DM-102): a slot the Endpoint does not give this person never
 * becomes a column or a label, because the rows read with their session are the projection.
 */
import { readableName } from "@joinedcontext/sdk";
import type { LinkmlSlot } from "../../pages/models/linkml";
// A byte copy of jc-core's data/field-order.json at the pinned platform rev (T-4661, T-5192): the one
// label table the assistant widget, the CKAN map popup and the publisher already read;
// tests/field_order_copy.rs keeps it equal to the pinned crate's.
import fieldOrder from "../../data/field-order.json";

const SHARED_LABELS = fieldOrder.labels as Record<string, Record<string, string>>;

/** Last unless ranked (DM-98). */
const SYSTEM = new Set(["id", "type", "location", "dateCreated", "dateModified"]);
/** What names an entity comes first among the attributes no model lists. */
const NAMING = ["name", "title", "alternateName", "description"];

/**
 * The display order: ranked slots, lower first; the class's slots as written; the system
 * attributes; then what the model does not list, naming attributes first and the rest by name.
 * Only attributes in `granted` appear.
 *
 * ponytail: the name heuristic for unlisted attributes is naming-first then alphabetical; the full
 * table is jc-core's field-order.json, which the Portal UI does not bundle yet.
 */
export function displayOrder(slots: readonly Pick<LinkmlSlot, "name" | "rank">[], granted: readonly string[]): string[] {
  const has = new Set(granted);
  const listed = slots.filter((slot) => has.has(slot.name));
  const ranked = listed
    .filter((slot) => slot.rank !== undefined)
    .sort((a, b) => (a.rank ?? 0) - (b.rank ?? 0))
    .map((slot) => slot.name);
  const unranked = listed.filter((slot) => slot.rank === undefined).map((slot) => slot.name);
  const own = unranked.filter((name) => !SYSTEM.has(name));
  const system = unranked.filter((name) => SYSTEM.has(name));
  const known = new Set([...ranked, ...unranked]);
  const rest = granted
    .filter((name) => !known.has(name))
    .sort((a, b) => {
      const ra = NAMING.indexOf(a);
      const rb = NAMING.indexOf(b);
      if (ra !== rb) return (ra === -1 ? NAMING.length : ra) - (rb === -1 ? NAMING.length : rb);
      const sa = SYSTEM.has(a) ? 1 : 0;
      const sb = SYSTEM.has(b) ? 1 : 0;
      return sa - sb || a.localeCompare(b);
    });
  return [...new Set([...ranked, ...own, ...system, ...rest])];
}

/** `availableBikeNumber` and `ngsi-ld:route_color` as a person reads them: "Available bike number". */
export function readable(attr: string): string {
  const words = readableName(attr)
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .trim()
    .toLowerCase();
  return words === "" ? attr : words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * An attribute's label (DM-100, T-5192): the slot's `annotations.label` in the language, in the
 * model's own language, in English; its title; the shared field-order.json label of a common
 * NGSI-LD attribute; the name made readable. Shown as text, never as markup.
 */
export function columnLabel(slot: Pick<LinkmlSlot, "label" | "title"> | undefined, attr: string, locale: string): string {
  const base = locale.split("-")[0];
  const pick = (map: Record<string, string> | undefined) =>
    map ? (map[locale] ?? map[base] ?? map[""] ?? map.en) : undefined;
  const shared = Object.hasOwn(SHARED_LABELS, attr) ? SHARED_LABELS[attr] : undefined;
  return pick(slot?.label) ?? pick(slot?.title) ?? pick(shared) ?? readable(attr);
}
