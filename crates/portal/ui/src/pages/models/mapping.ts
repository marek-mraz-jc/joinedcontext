/**
 * What a Mapping means, without any of the pixels (T-0223, DM-33, DM-34, DM-38, DM-40).
 *
 * A Mapping is a LinkML-Map `TransformationSpecification` and nothing else: no JSONata, no jq,
 * no JavaScript (DM-33). This module holds the three things the editor needs to be more than a
 * text box — where the first guess at a derivation comes from, what the derivations do to one
 * example, and what the set of them costs to review — and it holds them as pure functions so
 * the meaning can be tested without rendering anything.
 *
 * The preview interpreter here is deliberately partial. It evaluates the invertible subset the
 * gateway can also run (`populated_from`, `value_mappings`, linear `unit_conversion`, `cast`)
 * and it evaluates nothing else. `expr` and `native` blocks come back named rather than
 * guessed at, because the authority on those is Bento running the compiled Bloblang in CI
 * (DM-35, DM-39), and a browser that quietly produced a different answer would be teaching the
 * author something untrue about their own mapping.
 */

import type { LinkmlModel, LinkmlSlot } from "./linkml";

/** Where the editor's first guess at a derivation came from (DM-40). */
export type Origin = "identical" | "exact" | "close" | "alignment" | "manual";

/** One SSSOM row, as `datamodels/alignments/*.sssom.tsv` carries it (DM-42). */
export interface Alignment {
  subject_id: string;
  predicate_id: string;
  object_id: string;
}

/** A linear unit conversion: `value * factor + offset` (DM-36). */
export interface UnitConversion {
  factor: number;
  offset?: number;
  fromUnit?: string;
  toUnit?: string;
}

/** One target slot and how it is filled. */
export interface Derivation {
  /** The target slot this fills; a derivation always has exactly one (DM-38). */
  target: string;
  /** The source slot it reads, when it reads one. */
  populatedFrom?: string;
  /** Where this derivation came from, for the badge the editor shows. */
  origin: Origin;
  /** Source value to target value, for enum alignment (DM-36). */
  valueMappings?: Record<string, string>;
  /** A linear conversion between the two slots' units (DM-36). */
  unitConversion?: UnitConversion;
  /** The target range to coerce to (DM-36). */
  cast?: string;
  /**
   * Bloblang this editor does not read (DM-38). Its presence raises the lane and marks the
   * slot unchecked; the compile report is where it is judged, not here.
   */
  native?: string;
}

/** The review lane a change has to travel through (CC-32). */
export type Lane = "green" | "yellow" | "red";

/**
 * The editor's first guess at how to fill each target slot (DM-40).
 *
 * Three sources of evidence, strongest first, and each target slot takes the first that hits:
 * an identical slot name, the same `slot_uri` on both sides, and an SSSOM alignment set. The
 * order matters because they disagree: two models can align a slot loosely by SSSOM and also
 * happen to name it the same, and the name is the one the author meant.
 *
 * A slot with no evidence is returned with no source, because an unfilled target slot is
 * something the author has to see and decide about; guessing by string similarity would hide
 * exactly the slots that need attention.
 */
export function autoAlign(
  source: LinkmlModel,
  target: LinkmlModel,
  alignments: Alignment[] = [],
): Derivation[] {
  const byName = new Map(source.slots.map((slot) => [slot.name, slot]));
  const byUri = new Map(
    source.slots.flatMap((slot) => (slot.slot_uri ? [[slot.slot_uri, slot]] : [])),
  );
  const aligned = new Map<string, { source: string; origin: Origin }>();
  for (const row of alignments) {
    const from = source.slots.find((slot) => matches(slot, row.subject_id));
    const to = target.slots.find((slot) => matches(slot, row.object_id));
    if (!from || !to || aligned.has(to.name)) {
      continue;
    }
    aligned.set(to.name, {
      source: from.name,
      origin: row.predicate_id.endsWith("exactMatch") ? "exact" : "close",
    });
  }

  return target.slots.map((slot) => {
    const identical = byName.get(slot.name);
    if (identical) {
      return withConversion({ target: slot.name, populatedFrom: slot.name, origin: "identical" }, identical, slot);
    }
    const sameTerm = slot.slot_uri ? byUri.get(slot.slot_uri) : undefined;
    if (sameTerm) {
      return withConversion(
        { target: slot.name, populatedFrom: sameTerm.name, origin: "exact" },
        sameTerm,
        slot,
      );
    }
    const sssom = aligned.get(slot.name);
    if (sssom) {
      const from = byName.get(sssom.source);
      const derivation: Derivation = {
        target: slot.name,
        populatedFrom: sssom.source,
        origin: sssom.origin,
      };
      return from ? withConversion(derivation, from, slot) : derivation;
    }
    return { target: slot.name, origin: "manual" };
  });
}

/** Whether a slot is the one an SSSOM row names, by name or by its own term IRI. */
function matches(slot: LinkmlSlot, id: string): boolean {
  return slot.name === id || slot.slot_uri === id;
}

/**
 * A derivation that also carries the unit conversion the two slots imply.
 *
 * Only the fact that the units differ is inferred, never the factor: the editor asks for that.
 * A guessed factor is a silently wrong measurement, which is worse than an empty field.
 */
function withConversion(
  derivation: Derivation,
  from: LinkmlSlot,
  to: LinkmlSlot,
): Derivation {
  const fromUnit = from.unit?.ucum_code ?? from.unit?.symbol;
  const toUnit = to.unit?.ucum_code ?? to.unit?.symbol;
  if (!fromUnit || !toUnit || fromUnit === toUnit) {
    return derivation;
  }
  return { ...derivation, unitConversion: { factor: 1, fromUnit, toUnit } };
}

/**
 * Target slots that are required and nothing fills (DM-34).
 *
 * DM-40 refuses to save a Mapping that fails DM-34, and this is the half of DM-34 the editor
 * can decide on its own: whether every required target slot has a derivation. Type mismatches
 * and undeclared units are Model Tools' answer, against both real schemas.
 */
export function unfilledRequired(target: LinkmlModel, derivations: Derivation[]): string[] {
  const filled = new Set(
    derivations
      .filter((derivation) => derivation.populatedFrom || derivation.native)
      .map((derivation) => derivation.target),
  );
  return target.slots
    .filter((slot) => slot.required && !filled.has(slot.name))
    .map((slot) => slot.name);
}

/**
 * The lane this mapping has to travel through (DM-38).
 *
 * Every native block raises the lane by one, and one is enough: the point is that a human
 * reads Bloblang nobody validated, and a second block does not make that more true. `base` is
 * the lane the change would have had without them.
 */
export function laneOf(derivations: Derivation[], base: Lane = "green"): Lane {
  if (!derivations.some((derivation) => derivation.native?.trim())) {
    return base;
  }
  return base === "green" ? "yellow" : "red";
}

/** One example, transformed, and the slots the browser did not judge. */
export interface Transformed {
  /** The target entity, as far as the invertible subset can produce it. */
  output: Record<string, unknown>;
  /** Target slots whose value came from a native block and was not evaluated (DM-38). */
  unchecked: string[];
  /** Target slots whose source slot is not in the input at all. */
  missing: string[];
}

/**
 * One input example through the derivations (DM-40).
 *
 * The output is a plain object rather than a full NGSI-LD entity: this is the shape the author
 * is checking, and the entity envelope is the pipeline's business. `id` and `type` ride through
 * untouched because every entity has them and no derivation should have to say so.
 */
export function transform(
  input: Record<string, unknown>,
  derivations: Derivation[],
): Transformed {
  const output: Record<string, unknown> = {};
  const unchecked: string[] = [];
  const missing: string[] = [];

  for (const key of ["id", "type"]) {
    if (key in input) {
      output[key] = input[key];
    }
  }

  for (const derivation of derivations) {
    if (derivation.native?.trim()) {
      unchecked.push(derivation.target);
      continue;
    }
    if (!derivation.populatedFrom) {
      continue;
    }
    if (!(derivation.populatedFrom in input)) {
      missing.push(derivation.target);
      continue;
    }
    output[derivation.target] = apply(input[derivation.populatedFrom], derivation);
  }

  return { output, unchecked, missing };
}

/** One value through one derivation: remap, convert, coerce, in that order. */
function apply(value: unknown, derivation: Derivation): unknown {
  let current = value;

  if (derivation.valueMappings) {
    const key = String(current);
    // A value the author did not map is left as it stands rather than dropped: the editor
    // shows it, and Model Tools refuses it against the target's permissible_values (DM-34).
    current = key in derivation.valueMappings ? derivation.valueMappings[key] : current;
  }

  const conversion = derivation.unitConversion;
  if (conversion && typeof current === "number") {
    current = current * conversion.factor + (conversion.offset ?? 0);
  }

  return derivation.cast ? coerce(current, derivation.cast) : current;
}

/** A value in the target's range, for the ranges a cast can actually reach. */
function coerce(value: unknown, range: string): unknown {
  switch (range) {
    case "integer": {
      const number = Number(value);
      return Number.isFinite(number) ? Math.trunc(number) : value;
    }
    case "float":
    case "double":
    case "decimal": {
      const number = Number(value);
      return Number.isFinite(number) ? number : value;
    }
    case "boolean":
      return typeof value === "boolean" ? value : value === "true" || value === 1;
    case "string":
      return typeof value === "string" ? value : String(value);
    default:
      return value;
  }
}

/** YAML words that read as a boolean or null rather than as the text. */
const YAML_WORDS = /^(true|false|yes|no|on|off|y|n|null|~)$/i;

/**
 * One value as YAML reads it back: plain when it is a plain name, else double-quoted (JSON's
 * quoting is YAML's). A UCUM unit such as `[degF]`, or an enum value holding ": " or "#",
 * would otherwise read as another shape (F-UI2-012).
 */
function scalar(text: string): string {
  return /^[A-Za-z0-9_][A-Za-z0-9_./-]*$/.test(text) && !YAML_WORDS.test(text) ? text : JSON.stringify(text);
}

/**
 * The derivations as the `spec.transformation` of a `Mapping` manifest (DM-33).
 *
 * Written by hand rather than through a YAML library because the shape is fixed and small, and
 * because what is saved has to be readable in a pull request: this is a repository change a
 * person reviews, not a payload (CC-32).
 */
export function toTransformationSpec(
  derivations: Derivation[],
  sourceClass: string,
  targetClass: string,
): string {
  const lines = ["class_derivations:", `  ${scalar(targetClass)}:`, `    populated_from: ${scalar(sourceClass)}`];
  const filled = derivations.filter(
    (derivation) => derivation.populatedFrom || derivation.native,
  );
  if (filled.length === 0) {
    return `${lines.join("\n")}\n    slot_derivations: {}\n`;
  }
  lines.push("    slot_derivations:");
  for (const derivation of filled) {
    lines.push(`      ${scalar(derivation.target)}:`);
    if (derivation.native?.trim()) {
      // The block itself lives in `spec.native[]`, attached to this one target slot; the
      // derivation only records that something else fills it (DM-38).
      lines.push(`        # native: bloblang, unchecked (DM-38)`);
      continue;
    }
    lines.push(`        populated_from: ${scalar(derivation.populatedFrom ?? "")}`);
    if (derivation.unitConversion) {
      const { factor, offset, fromUnit, toUnit } = derivation.unitConversion;
      lines.push("        unit_conversion:");
      if (fromUnit) {
        lines.push(`          source_unit: ${scalar(fromUnit)}`);
      }
      if (toUnit) {
        lines.push(`          target_unit: ${scalar(toUnit)}`);
      }
      lines.push(`          factor: ${factor}`);
      if (offset) {
        lines.push(`          offset: ${offset}`);
      }
    }
    if (derivation.valueMappings && Object.keys(derivation.valueMappings).length > 0) {
      lines.push("        value_mappings:");
      for (const [from, to] of Object.entries(derivation.valueMappings)) {
        lines.push(`          ${scalar(from)}: ${scalar(String(to))}`);
      }
    }
    if (derivation.cast) {
      lines.push(`        range: ${scalar(derivation.cast)}`);
    }
  }
  return `${lines.join("\n")}\n`;
}

/** The native blocks of a mapping, as `spec.native[]` records them (DM-38). */
export function nativeBlocks(
  derivations: Derivation[],
): { targetSlot: string; language: "bloblang"; source: string }[] {
  return derivations
    .filter((derivation): derivation is Derivation & { native: string } =>
      Boolean(derivation.native?.trim()),
    )
    .map((derivation) => ({
      targetSlot: derivation.target,
      language: "bloblang",
      source: derivation.native,
    }));
}
