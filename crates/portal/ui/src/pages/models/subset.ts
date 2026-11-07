import { edit, parseModel } from "./linkml";
import type { LinkmlModel } from "./linkml";

/**
 * A subset of a model: which classes, and per class which slots (MP-01).
 *
 * This is the shape `ModelProjection.spec.classes` takes, so what the picker produces is what
 * the endpoint manifest carries. A class that is absent is not exposed at all; a class listed
 * with no slots is exposed with its identity only — the two are different on purpose.
 */
export interface SubsetClass {
  name: string;
  slots: string[];
}

export interface Subset {
  classes: SubsetClass[];
}

/** What every NGSI-LD entity carries whatever the projection says; never unticked. */
export const IDENTITY_SLOTS = ["id", "type"];

export const EMPTY_SUBSET: Subset = { classes: [] };

/** Everything the model has, which is where a new projection starts. */
export function wholeSubset(model: LinkmlModel): Subset {
  return { classes: model.classes.map((klass) => ({ name: klass.name, slots: [...klass.slots] })) };
}

/**
 * Every class or slot the subset names that the model does not have (MP-01).
 *
 * Empty means the subset is a narrowing of this model. The same check refuses the manifest in
 * CI (T-0563), so a person sees here what the pipeline would say later.
 */
export function subsetProblems(model: LinkmlModel, subset: Subset): string[] {
  const problems: string[] = [];
  const seen = new Set<string>();
  for (const chosen of subset.classes) {
    if (seen.has(chosen.name)) {
      problems.push(`class '${chosen.name}' is listed twice`);
      continue;
    }
    seen.add(chosen.name);
    const klass = model.classes.find((candidate) => candidate.name === chosen.name);
    if (!klass) {
      problems.push(`unknown class '${chosen.name}'`);
      continue;
    }
    for (const slot of chosen.slots) {
      if (!klass.slots.includes(slot)) {
        problems.push(`class '${chosen.name}' has no slot '${slot}'`);
      }
    }
  }
  return problems;
}

/** The subset with `klass` present (ids only) or absent. */
export function toggleClass(subset: Subset, klass: string, present: boolean): Subset {
  const rest = subset.classes.filter((chosen) => chosen.name !== klass);
  return { classes: present ? [...rest, { name: klass, slots: [] }] : rest };
}

/** The subset with `slot` of `klass` ticked or not; ticking a slot ticks its class. */
export function toggleSlot(subset: Subset, klass: string, slot: string, present: boolean): Subset {
  const rest = subset.classes.filter((candidate) => candidate.name !== klass);
  const chosen = subset.classes.find((candidate) => candidate.name === klass) ?? {
    name: klass,
    slots: [],
  };
  const slots = present
    ? chosen.slots.includes(slot)
      ? chosen.slots
      : [...chosen.slots, slot]
    : chosen.slots.filter((candidate) => candidate !== slot);
  return { classes: [...rest, { name: klass, slots }] };
}

/**
 * The model narrowed to the subset: the chosen classes with their chosen slots (identity slots
 * always kept), the slots those classes still use, and the enums those slots still range over.
 *
 * Rendered through the YAML document so the source keeps its prefixes, comments and order; it
 * is what the preview compiles in subset mode, so a person sees the model a consumer would
 * fetch from the endpoint (MP-03), not the one they drew.
 */
export function subsetSource(source: string, subset: Subset): string {
  const model = parseModel(source);
  const chosen = new Map(subset.classes.map((klass) => [klass.name, klass.slots]));
  const keptSlots = new Set<string>();
  return edit(source, (document) => {
    for (const klass of model.classes) {
      const picked = chosen.get(klass.name);
      if (picked === undefined) {
        document.deleteIn(["classes", klass.name]);
        continue;
      }
      const slots = klass.slots.filter(
        (slot) => picked.includes(slot) || IDENTITY_SLOTS.includes(slot),
      );
      for (const slot of slots) {
        keptSlots.add(slot);
      }
      document.setIn(["classes", klass.name, "slots"], slots);
    }
    const keptEnums = new Set(
      model.slots.filter((slot) => keptSlots.has(slot.name)).map((slot) => slot.range),
    );
    for (const slot of model.slots) {
      if (!keptSlots.has(slot.name)) {
        document.deleteIn(["slots", slot.name]);
      }
    }
    for (const entry of model.enums) {
      if (!keptEnums.has(entry.name)) {
        document.deleteIn(["enums", entry.name]);
      }
    }
  });
}
