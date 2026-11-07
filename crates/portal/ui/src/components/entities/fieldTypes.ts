/**
 * The field types a data view offers (T-3098, ADR-N-042 §3.1): each one a LinkML range and an
 * NGSI-LD kind, so adding a field is a DataModel Change made of the model editor's own operations,
 * never a column bolted onto a table.
 */
import { useTranslation } from "react-i18next";
import type { NgsiLdKind, LinkmlModel, LinkmlSlot } from "../../pages/models/linkml";
import { classSlots, edit, isAnyClass, RANGES, UPSTREAM_ANNOTATION, valueClasses } from "../../pages/models/linkml";
import type { Applied, Operation } from "../../pages/models/operations";
import { applyOperations, isName } from "../../pages/models/operations";
import { checkFormulas, parseFormula, rangeOf } from "./formula";
import type { ClassSlot } from "./formula";

export const FIELD_TYPES = [
  "text",
  "number",
  "integer",
  "date",
  "datetime",
  "boolean",
  "select",
  "multiSelect",
  "url",
  "email",
  "file",
  "location",
  "relationship",
  "formula",
] as const;

export type FieldType = (typeof FIELD_TYPES)[number];

/** What a field type is in the model: its range (a select's is its own enum) and its NGSI-LD kind. */
export const FIELD_RANGE: Record<FieldType, { range?: string; kind: NgsiLdKind }> = {
  text: { range: "string", kind: "Property" },
  number: { range: "float", kind: "Property" },
  integer: { range: "integer", kind: "Property" },
  date: { range: "date", kind: "Property" },
  datetime: { range: "datetime", kind: "Property" },
  boolean: { range: "boolean", kind: "Property" },
  select: { kind: "Property" },
  multiSelect: { kind: "Property" },
  url: { range: "uri", kind: "Property" },
  email: { range: "string", kind: "Property" },
  // A link to a file the space keeps elsewhere: its address, typed as one.
  file: { range: "uri", kind: "Property" },
  location: { kind: "GeoProperty" },
  relationship: { kind: "Relationship" },
  // Computed from the entity's other slots by the class's formula pipeline (DM-80): its range is
  // what the formula yields.
  formula: { range: "float", kind: "Property" },
};

/**
 * What a slot is, in the words of the Add field dialog (T-4829): a steward reads "Number" or
 * "Link to Building", never `float` or a range IRI. `any` is a slot over LinkML `Any`; `object` a
 * value object written inside the entity, with the fields it holds.
 */
export type FieldWord =
  | { type: FieldType; target?: string }
  | { type: "any" }
  | { type: "object"; target: string; fields: string[] };

/** A field's type in plain words (T-4829): "Number", "Link to Building", "any value". */
export function useFieldWords(): (word: FieldWord) => string {
  const { t } = useTranslation();
  return (word) =>
    word.type === "any"
      ? t("models.word.any")
      : word.type === "object"
        ? t("models.word.object", { fields: word.fields.join(", ") })
        : word.type === "relationship" && word.target
          ? t("models.word.linkTo", { target: word.target })
          : t(`spaces.fields.types.${word.type}`);
}

const RANGE_WORD: Record<string, FieldType> = {
  float: "number",
  double: "number",
  decimal: "number",
  integer: "integer",
  date: "date",
  datetime: "datetime",
  boolean: "boolean",
  uri: "url",
  uriorcurie: "url",
};

export function fieldWord(slot: LinkmlSlot, model: LinkmlModel): FieldWord {
  if (slot.kind === "GeoProperty") return { type: "location" };
  if (slot.equals_expression) return { type: "formula" };
  const klass = slot.range === undefined ? undefined : model.classes.find((one) => one.name === slot.range);
  if (klass && isAnyClass(klass)) return { type: "any" };
  if (klass && valueClasses(model).has(klass.name)) {
    return { type: "object", target: klass.name, fields: classSlots(model, klass).map((one) => one.name) };
  }
  if (klass || slot.kind === "Relationship") return { type: "relationship", target: slot.range };
  if (slot.range !== undefined && model.enums.some((one) => one.name === slot.range)) {
    return { type: slot.multivalued ? "multiSelect" : "select" };
  }
  if (slot.pattern === EMAIL_PATTERN) return { type: "email" };
  return { type: RANGE_WORD[slot.range ?? model.default_range ?? "string"] ?? "text" };
}

/** An address with one `@` and a dot after it: the same check the grid makes at the cell. */
export const EMAIL_PATTERN = "^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$";

export interface FieldDraft {
  name: string;
  type: FieldType;
  required: boolean;
  /** A select's choices, one per value. */
  values: string[];
  /** A relationship's target class and the name of the slot there that points back. */
  target: string;
  inverse: string;
  /** A relationship to several entities, not one. */
  many: boolean;
  /** A formula field's `equals_expression` (DM-80). */
  expression: string;
}

export const EMPTY_DRAFT: FieldDraft = {
  name: "",
  type: "text",
  required: false,
  values: [],
  target: "",
  inverse: "",
  many: false,
  expression: "",
};

/** What is wrong with a draft, by the input that must change; empty when it can be proposed. */
export type DraftProblems = Partial<Record<"name" | "values" | "target" | "inverse" | "expression", "nameInvalid" | "nameTaken" | "valuesMissing" | "valueInvalid" | "valueTwice" | "targetMissing" | "inverseInvalid" | "inverseTaken" | "formulaInvalid">>;

/** The names a class answers to already: its own slots, inline or listed, and the model's slots. */
function takenNames(model: LinkmlModel, type: string): Set<string> {
  const cls = model.classes.find((c) => c.name === type);
  return new Set([...(cls ? classSlots(model, cls).map((s) => s.name) : []), ...model.slots.map((s) => s.name)]);
}

export function problemsOf(draft: FieldDraft, model: LinkmlModel, type: string): DraftProblems {
  const problems: DraftProblems = {};
  if (!isName(draft.name)) problems.name = "nameInvalid";
  else if (takenNames(model, type).has(draft.name)) problems.name = "nameTaken";
  if (draft.type === "select" || draft.type === "multiSelect") {
    if (draft.values.length === 0) problems.values = "valuesMissing";
    else if (draft.values.some((value) => !isValue(value))) problems.values = "valueInvalid";
    else if (new Set(draft.values).size !== draft.values.length) problems.values = "valueTwice";
  }
  if (draft.type === "formula" && formulaProblem(draft, model, type) !== undefined) problems.expression = "formulaInvalid";
  if (draft.type === "relationship") {
    if (!model.classes.some((c) => c.name === draft.target)) problems.target = "targetMissing";
    if (!isName(draft.inverse) || draft.inverse === draft.name) problems.inverse = "inverseInvalid";
    else if (takenNames(model, draft.target).has(draft.inverse)) problems.inverse = "inverseTaken";
  }
  return problems;
}

/** The class's slots for the formula checks, with the draft's formula among them (DM-80). */
export function formulaSlots(draft: FieldDraft, model: LinkmlModel, type: string): ClassSlot[] {
  const cls = model.classes.find((c) => c.name === type);
  const own: ClassSlot[] = (cls ? classSlots(model, cls) : []).map((slot) => ({
    name: slot.name,
    kind: slot.kind,
    ...(slot.equals_expression !== undefined ? { equals_expression: slot.equals_expression } : {}),
  }));
  return draft.type === "formula" ? [...own, { name: draft.name, kind: "Property", equals_expression: draft.expression }] : own;
}

/** Why the draft's formula cannot be computed, in the checker's words; `undefined` when it can. */
export function formulaProblem(draft: FieldDraft, model: LinkmlModel, type: string): string | undefined {
  if (draft.type !== "formula") return undefined;
  if (draft.expression.trim() === "") return "the formula is empty";
  return checkFormulas(formulaSlots(draft, model, type)).problems[draft.name];
}

/** The model with the slot's `equals_expression` written, beside the operations that added it. */
export function withFormula(applied: Applied, name: string, expression: string): Applied {
  if (applied.refused.length > 0) return applied;
  return {
    refused: [],
    source: edit(applied.source, (document) => {
      document.setIn(["slots", name, "equals_expression"], expression.trim());
    }),
  };
}

/** A choice as it is stored: trimmed text of up to 100 characters, no control characters. */
function isValue(value: string): boolean {
  const control = [...value].some((char) => char.charCodeAt(0) < 0x20 || char.charCodeAt(0) === 0x7f);
  return value !== "" && value === value.trim() && value.length <= 100 && !control;
}

/** `capacityLevel` → `CapacityLevel`: the enum a select field brings is named after it. */
function enumNameOf(type: string, field: string): string {
  return `${type}${field.charAt(0).toUpperCase()}${field.slice(1)}`;
}

/** The model operations one checked draft is: what the Change proposes. */
export function fieldOperations(draft: FieldDraft, model: LinkmlModel, type: string): Operation[] {
  if (draft.type === "relationship") {
    return [
      {
        op: "addRelationship",
        from: type,
        to: draft.target,
        name: draft.name,
        inverse: draft.inverse,
        // The field holds one target or several; the other end lists every entity pointing at it.
        cardinality: draft.many ? "many-to-many" : "many-to-one",
        required: draft.required || undefined,
      },
    ];
  }
  const ops: Operation[] = [];
  let range = FIELD_RANGE[draft.type].range;
  if (draft.type === "select" || draft.type === "multiSelect") {
    let name = enumNameOf(type, draft.name);
    while (model.enums.some((e) => e.name === name)) name = `${name}_`;
    range = name;
    ops.push({ op: "addEnum", name }, ...draft.values.map((value): Operation => ({ op: "addEnumValue", enum: name, value })));
  }
  if (draft.type === "formula") range = rangeOf(parseFormula(draft.expression));
  const kind = FIELD_RANGE[draft.type].kind;
  ops.push({ op: "addSlot", name: draft.name, class: type, range, ...(kind === "Property" ? {} : { kind }) });
  if (draft.type === "multiSelect") ops.push({ op: "setSlot", name: draft.name, field: "multivalued", value: true });
  if (draft.type === "email") ops.push({ op: "setSlot", name: draft.name, field: "pattern", value: EMAIL_PATTERN });
  // A formula has a value when what it reads has one; required would refuse every entity without.
  if (draft.required && draft.type !== "formula") ops.push({ op: "setSlot", name: draft.name, field: "required", value: true });
  return ops;
}

/**
 * A Smart Data Model attribute offered first (T-3098): a slot of the official model's class of the
 * same name, which the type does not have yet and whose range a field can hold (a standard range,
 * one of that model's enums, or a geometry). Relationships to other official types are left to the
 * model editor, where the target class is added with them.
 */
export function suggestedSlots(official: LinkmlModel, model: LinkmlModel, type: string): LinkmlSlot[] {
  const cls = official.classes.find((c) => c.name === type);
  if (!cls) return [];
  const taken = takenNames(model, type);
  return classSlots(official, cls).filter(
    (slot) =>
      !taken.has(slot.name) &&
      isName(slot.name) &&
      (slot.kind === "GeoProperty" ||
        (slot.kind !== "Relationship" &&
          (slot.range === undefined ||
            (RANGES as readonly string[]).includes(slot.range) ||
            official.enums.some((e) => e.name === slot.range)))),
  );
}

/**
 * The model with one official slot copied into the type: its enum, range, kind and multiplicity by
 * the model editor's operations, then its IRI and description as a citation of where it comes from
 * (`upstream_source`, DM-58), so the reserved Smart Data Models IRI is accepted as the standard's
 * own term and its meaning stays the upstream's.
 */
export function withOfficialSlot(source: string, slot: LinkmlSlot, official: LinkmlModel, model: LinkmlModel, type: string): Applied {
  const ops: Operation[] = [];
  let range = slot.kind === "GeoProperty" ? undefined : slot.range;
  const officialEnum = official.enums.find((e) => e.name === slot.range);
  if (officialEnum) {
    if (!model.enums.some((e) => e.name === officialEnum.name)) {
      ops.push(
        { op: "addEnum", name: officialEnum.name },
        ...officialEnum.permissible_values.map(
          (value): Operation => ({ op: "addEnumValue", enum: officialEnum.name, value: value.name, description: value.description }),
        ),
      );
    }
    range = officialEnum.name;
  }
  ops.push({ op: "addSlot", name: slot.name, class: type, range, ...(slot.kind === "Property" ? {} : { kind: slot.kind }) });
  if (slot.multivalued) ops.push({ op: "setSlot", name: slot.name, field: "multivalued", value: true });
  const applied = applyOperations(source, ops);
  if (applied.refused.length > 0) return applied;
  const upstream = slot.upstream ?? official.id;
  return {
    refused: [],
    source: edit(applied.source, (document) => {
      if (upstream) {
        document.setIn(["slots", slot.name, "annotations", UPSTREAM_ANNOTATION], upstream);
        if (slot.slot_uri) document.setIn(["slots", slot.name, "slot_uri"], slot.slot_uri);
      }
      if (slot.description) document.setIn(["slots", slot.name, "description"], slot.description);
    }),
  };
}
