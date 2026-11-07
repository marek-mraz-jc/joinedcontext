import { isMap, isScalar } from "yaml";
import type { Document } from "yaml";
import {
  CARDINALITIES,
  DEFAULT_KIND,
  EMPTY_MODEL,
  NGSI_LD_KINDS,
  ON_DELETE_RULES,
  RANGES,
  edit,
  flagsOf,
  mintIri,
  parseModel,
  relationships,
  reservedNamespace,
  setOrDelete,
  storedEnd,
} from "./linkml";
import type { Cardinality, LinkmlModel, LinkmlSlot, NgsiLdKind, OnDelete, Relationship } from "./linkml";
import { unitOf } from "../../units";

/**
 * The operations the editor can perform on a model, as plain data (DM-13, DM-31, DM-32).
 *
 * The structured view, a test and an assistant all edit the document through this one set: a
 * person clicks and the view emits an operation; an assistant answers with a list of them and
 * the Portal applies it the same way. Every operation is refused by name when it points at a
 * class, slot or enum the model does not have, so a typo can never quietly edit nothing, and a
 * slot IRI under a reserved namespace is refused here rather than only flagged later (DM-16).
 */
export type Operation =
  | {
      op: "addClass";
      name: string;
      class_uri?: string;
      description?: string;
      /** The parent class; `Entity` makes it an NGSI-LD entity type (DM-09). */
      is_a?: string;
    }
  | { op: "removeClass"; name: string }
  /** Renames the class in every `is_a` and every slot `range` that names it. */
  | { op: "renameClass"; name: string; to: string }
  | { op: "setClass"; name: string; field: "class_uri" | "description" | "is_a"; value: string }
  /** The classes a class mixes in; an empty list removes the key (DM-13). */
  | { op: "setClassMixins"; name: string; mixins: string[] }
  /** The profiles a slot belongs to; an empty list removes the key. */
  | { op: "setSlotSubsets"; name: string; subsets: string[] }
  | {
      op: "addSlot";
      name: string;
      /** The class the slot is attached to; a slot without one is declared and left loose. */
      class?: string;
      range?: string;
      slot_uri?: string;
      kind?: NgsiLdKind;
    }
  | { op: "removeSlot"; name: string }
  /** Renames the slot in every class that lists it, keeping its place there. */
  | { op: "renameSlot"; name: string; to: string }
  | { op: "attachSlot"; class: string; slot: string }
  | { op: "detachSlot"; class: string; slot: string }
  | { op: "setSlot"; name: string; field: SlotField; value: unknown }
  | { op: "setTitle"; target: "class" | "slot"; name: string; locale: string; value: string }
  | { op: "addEnum"; name: string }
  | { op: "addEnumValue"; enum: string; value: string; description?: string }
  /**
   * What a person reads for one permissible value (UI-86): its title in one language, or its
   * description. An empty text removes it.
   */
  | { op: "setEnumValue"; enum: string; value: string; field: "title" | "description"; locale?: string; text: string }
  | { op: "removeEnumValue"; enum: string; value: string }
  /** Renames the enumeration in every slot `range` that names it. */
  | { op: "renameEnum"; name: string; to: string }
  /** Renames a stored value, keeping its labels; rows already stored keep the old value. */
  | { op: "renameEnumValue"; enum: string; value: string; to: string }
  /**
   * A relationship, both ends in one step (DM-64): `name` on `from` points at `to`, `inverse` on
   * `to` points back. The cardinality is read from `from` to `to`; `required` is allowed on the
   * stored end only (DM-65), and `onDelete` is written on the source, `restrict` by default.
   */
  | {
      op: "addRelationship";
      from: string;
      to: string;
      name: string;
      inverse: string;
      cardinality: Cardinality;
      required?: boolean;
      inverseRequired?: boolean;
      onDelete?: OnDelete;
    }
  /** Both ends' `multivalued` together; `name` is either end. */
  | { op: "setCardinality"; name: string; cardinality: Cardinality }
  /** The delete rule, on the source end; `name` is either end. */
  | { op: "setOnDelete"; name: string; onDelete: OnDelete }
  /** Both ends from every class and from the model; `name` is either end. */
  | { op: "removeRelationship"; name: string }
  /**
   * The fix for a relationship saved before inverses were required (DM-73): a multivalued
   * inverse on the target class, so the stored end stays the slot that holds the data today.
   */
  | { op: "addInverse"; name: string; inverse: string };

/** `unit` takes a UN/CEFACT common code (DM-06); `kind` an NGSI-LD kind; the rest their YAML value. */
export type SlotField =
  | "range"
  | "required"
  | "multivalued"
  | "deprecated"
  | "description"
  | "slot_uri"
  | "kind"
  | "unit"
  | "pattern"
  | "minimum_value"
  | "maximum_value";

export interface Refusal {
  /** Which operation of the list, so an assistant can correct that one. */
  index: number;
  reason: string;
}

export interface Applied {
  /** The new source, or the source that came in when anything was refused. */
  source: string;
  refused: Refusal[];
}

/**
 * LinkML element names: what the generators accept as a class, slot or enum name.
 *
 * ASCII, because a class name becomes the entity type in every URN the space mints
 * (`urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}`) and the gateway accepts a type matching
 * `^[A-Za-z][A-Za-z0-9_-]*$`. A model whose class is "Ovzduší" would mint ids its own endpoint
 * refuses. The human-readable name is the element's `title`, which is a LanguageProperty and
 * says "Ovzdušie" in as many languages as the space needs (T-1091).
 */
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Whether `name` can be a class or slot name: the rule `requireName` refuses on, for forms to check first. */
export function isName(name: string): boolean {
  return NAME.test(name);
}

class Refused extends Error {}

function refuse(reason: string): never {
  throw new Refused(reason);
}

function requireName(name: string, what: string): void {
  if (!NAME.test(name)) {
    // The reason, not only the refusal: a person typing an accented name is told where the
    // accented name belongs rather than left guessing which character offended (T-1091).
    refuse(
      `'${name}' is not a valid ${what} name: letters, digits and _ only, starting with a letter, ` +
        `because the name becomes the entity type in every URN. Put the readable name in its title.`,
    );
  }
}

function classOf(model: LinkmlModel, name: string) {
  return model.classes.find((klass) => klass.name === name) ?? refuse(`unknown class '${name}'`);
}

function slotOf(model: LinkmlModel, name: string) {
  return model.slots.find((slot) => slot.name === name) ?? refuse(`unknown slot '${name}'`);
}

function enumOf(model: LinkmlModel, name: string) {
  return model.enums.find((entry) => entry.name === name) ?? refuse(`unknown enum '${name}'`);
}

/** Where one permissible value lives in the document; refused for a value the enum lacks. */
function enumValuePath(model: LinkmlModel, name: string, value: string): string[] {
  const entry = enumOf(model, name);
  if (!entry.permissible_values.some((candidate) => candidate.name === value)) {
    refuse(`enum '${entry.name}' has no value '${value}'`);
  }
  return ["enums", entry.name, "permissible_values", value];
}

/** The fields whose value the document takes as it is, once it has the right type. */
const BOOLEAN_FIELDS: SlotField[] = ["required", "multivalued", "deprecated"];
const TEXT_FIELDS: SlotField[] = ["description", "pattern"];
const NUMBER_FIELDS: SlotField[] = ["minimum_value", "maximum_value"];

/**
 * What a borrowed term means is the upstream's to say, not ours (DM-58).
 *
 * A slot that cites an `upstream_source` has bound somebody else's IRI: the claim is "this slot
 * *is* that term". Changing what it holds — its type, its unit, whether it is one value or many,
 * whether it is required — while keeping the IRI does not extend a standard, it makes two
 * incompatible things answer to one name, and the disagreement surfaces at the federation
 * partner rather than here. Adding a term of our own beside it is always allowed; changing a
 * borrowed one is not.
 *
 * Checked at the edit, where both the citation and the change are in hand. Nothing downstream
 * can do it: the generators see one document with no history, and fetching the upstream to
 * compare would put a network call in the path of every keystroke's preview.
 */
const BORROWED_FIELDS: SlotField[] = ["range", "unit", "required", "multivalued"];

function refuseRedefinition(slot: LinkmlSlot, field: SlotField): void {
  if (slot.upstream && BORROWED_FIELDS.includes(field)) {
    refuse(
      `slot '${slot.name}' cites ${slot.upstream}, so '${field}' is that term's to define (DM-58); ` +
        `add a slot of your own under this model's prefix, or drop the citation and mint your own IRI`,
    );
  }
}

/** The relationship a slot is an end of, when its two ends agree. */
function relationshipOf(model: LinkmlModel, name: string): Relationship | undefined {
  return relationships(model).find((one) => one.source.slot === name || one.target.slot === name);
}

function relationshipNamed(model: LinkmlModel, name: string): Relationship {
  slotOf(model, name);
  return relationshipOf(model, name) ?? refuse(`slot '${name}' is not an end of a relationship`);
}

/** What only the relationship operations change on an end, so both ends change together. */
const RELATIONSHIP_FIELDS: SlotField[] = ["range", "kind", "multivalued"];

function refuseOneSidedEdit(model: LinkmlModel, slot: LinkmlSlot, field: SlotField, value: unknown): void {
  const relationship = relationshipOf(model, slot.name);
  if (relationship === undefined) {
    return;
  }
  if (RELATIONSHIP_FIELDS.includes(field)) {
    refuse(
      `slot '${slot.name}' is an end of the relationship ${relationship.source.class}.${relationship.source.slot} ↔ ` +
        `${relationship.target.class}.${relationship.target.slot}; change its cardinality with setCardinality, ` +
        `or remove the relationship with removeRelationship`,
    );
  }
  const computed = relationship.stored === "source" ? relationship.target : relationship.source;
  if (field === "required" && value === true && computed.slot === slot.name) {
    refuse(
      `slot '${slot.name}' is computed on read from ${relationship.stored === "source" ? relationship.source.slot : relationship.target.slot} ` +
        `and cannot be required; make the stored end required (DM-65)`,
    );
  }
}

function setSlotField(
  document: Document,
  model: LinkmlModel,
  name: string,
  field: SlotField,
  value: unknown,
): void {
  const slot = slotOf(model, name);
  refuseRedefinition(slot, field);
  refuseOneSidedEdit(model, slot, field, value);
  const path = ["slots", name];
  if (BOOLEAN_FIELDS.includes(field)) {
    if (typeof value !== "boolean") {
      refuse(`'${field}' of slot '${name}' takes true or false`);
    }
    setOrDelete(document, [...path, field], value);
    return;
  }
  if (TEXT_FIELDS.includes(field)) {
    if (typeof value !== "string") {
      refuse(`'${field}' of slot '${name}' takes text`);
    }
    setOrDelete(document, [...path, field], value);
    return;
  }
  if (NUMBER_FIELDS.includes(field)) {
    if (value !== undefined && value !== null && typeof value !== "number") {
      refuse(`'${field}' of slot '${name}' takes a number`);
    }
    setOrDelete(document, [...path, field], value ?? undefined);
    return;
  }
  if (typeof value !== "string") {
    refuse(`'${field}' of slot '${name}' takes text`);
  }
  const trimmed = value.trim();
  switch (field) {
    case "range": {
      const known = [
        ...RANGES,
        ...model.enums.map((entry) => entry.name),
        ...model.classes.map((klass) => klass.name),
      ];
      if (trimmed && !known.includes(trimmed)) {
        refuse(`range '${trimmed}' of slot '${name}' is neither a type, an enum nor a class of this model`);
      }
      setOrDelete(document, [...path, "range"], trimmed);
      return;
    }
    case "slot_uri": {
      const reserved = trimmed ? reservedNamespace(trimmed, model.prefixes) : undefined;
      if (reserved && !slot.upstream) {
        refuse(`slot '${name}' would mint '${trimmed}' under ${reserved}, which belongs to someone else`);
      }
      setOrDelete(document, [...path, "slot_uri"], trimmed);
      return;
    }
    case "kind": {
      if (!(NGSI_LD_KINDS as readonly string[]).includes(trimmed)) {
        refuse(`kind '${trimmed}' of slot '${name}' is not one of ${NGSI_LD_KINDS.join(", ")}`);
      }
      setOrDelete(
        document,
        [...path, "annotations", "ngsi_ld_kind"],
        trimmed === DEFAULT_KIND ? undefined : trimmed,
      );
      return;
    }
    case "unit": {
      if (!trimmed) {
        document.deleteIn([...path, "unit"]);
        return;
      }
      const unit = unitOf(trimmed);
      if (!unit) {
        refuse(
          `unit '${trimmed}' of slot '${name}' is not a UN/CEFACT Recommendation 20 code; codes are case-sensitive, such as GQ for µg/m³ or CEL for °C`,
        );
      }
      // The wire value and the anchor together (DM-06, DM-59): NGSI-LD puts the UN/CEFACT code
      // on the wire as `unitCode`, and the QUDT unit is what a federated reader dereferences
      // to align this measurement with somebody else's. The quantity kind says what dimension
      // is being measured, which is what makes two units comparable at all.
      // Every field comes from the code list; one it does not carry is left out, not guessed.
      document.setIn([...path, "unit"], {
        ...(unit.ucum ? { ucum_code: unit.ucum } : {}),
        exact_mappings: [`ucefact:${unit.code}`, ...(unit.qudt ? [`qudt-unit:${unit.qudt}`] : [])],
        ...(unit.quantityKinds.length > 0 ? { has_quantity_kind: `qudt-quantkind:${unit.quantityKinds[0]}` } : {}),
      });
      declarePrefixes(document);
      return;
    }
    default:
      refuse(`'${field}' is not a slot field the editor sets`);
  }
}

/**
 * The prefixes a unit mapping cites, declared on the document that carries one (DM-59).
 *
 * A CURIE under an undeclared prefix is a dangling string in every artifact that carries it,
 * and Model Tools refuses to render the model at all — so the editor writes the declaration
 * with the mapping rather than leaving the author to find out at generation time.
 */
function declarePrefixes(document: Document): void {
  for (const [prefix, namespace] of [
    ["ucefact", "https://vocabulary.uncefact.org/UnitMeasureCode#"],
    ["qudt-unit", "http://qudt.org/vocab/unit/"],
    ["qudt-quantkind", "http://qudt.org/vocab/quantitykind/"],
  ]) {
    if (document.getIn(["prefixes", prefix]) === undefined) {
      document.setIn(["prefixes", prefix], namespace);
    }
  }
}

function classSlots(document: Document, name: string): string[] {
  const listed = document.getIn(["classes", name, "slots"], true);
  const values = (listed as { toJSON?: () => unknown } | undefined)?.toJSON?.();
  return Array.isArray(values) ? values.filter((s): s is string => typeof s === "string") : [];
}

/** Renames a key of a top-level section in place, so its value, its comments and its position stay. */
function renameKey(document: Document, section: string | string[], name: string, to: string): void {
  const entries = document.getIn(typeof section === "string" ? [section] : section, true);
  const pair = isMap(entries)
    ? entries.items.find((item) => (isScalar(item.key) ? item.key.value : item.key) === name)
    : undefined;
  if (pair && isScalar(pair.key)) {
    pair.key.value = to;
  }
}

function mutate(document: Document, model: LinkmlModel, operation: Operation): void {
  switch (operation.op) {
    case "addClass": {
      requireName(operation.name, "class");
      if (model.classes.some((klass) => klass.name === operation.name)) {
        refuse(`class '${operation.name}' already exists`);
      }
      // Without an IRI of its own the class gets the organisation's (T-4829): nobody has to type one.
      const classUri = operation.class_uri ?? mintIri(model, operation.name);
      document.setIn(["classes", operation.name], {
        ...(classUri ? { class_uri: classUri } : {}),
        ...(operation.description ? { description: operation.description } : {}),
        ...(operation.is_a ? { is_a: operation.is_a } : {}),
        slots: [],
      });
      return;
    }
    case "removeClass": {
      classOf(model, operation.name);
      const held = relationships(model).filter(
        (one) => one.source.class === operation.name || one.target.class === operation.name,
      );
      if (held.length > 0) {
        refuse(
          `class '${operation.name}' is an end of ${held.map((one) => one.source.slot).join(", ")}; ` +
            `remove those relationships first, so no slot is left pointing at nothing`,
        );
      }
      document.deleteIn(["classes", operation.name]);
      return;
    }
    case "renameClass": {
      classOf(model, operation.name);
      requireName(operation.to, "class");
      if (model.classes.some((klass) => klass.name === operation.to)) {
        refuse(`class '${operation.to}' already exists`);
      }
      renameKey(document, "classes", operation.name, operation.to);
      for (const klass of model.classes) {
        const name = klass.name === operation.name ? operation.to : klass.name;
        if (document.getIn(["classes", name, "is_a"]) === operation.name) {
          document.setIn(["classes", name, "is_a"], operation.to);
        }
      }
      for (const slot of model.slots) {
        if (slot.range === operation.name) {
          document.setIn(["slots", slot.name, "range"], operation.to);
        }
      }
      return;
    }
    case "setClass":
      classOf(model, operation.name);
      // A class can only specialise one this model declares, or the hierarchy names nothing.
      if (operation.field === "is_a" && operation.value.trim() !== "") {
        const parent = operation.value.trim();
        if (parent === operation.name) {
          refuse(`class '${operation.name}' cannot specialise itself`);
        }
        classOf(model, parent);
      }
      // An IRI never has whitespace; a description is typed, and trimming would eat every space.
      setOrDelete(
        document,
        ["classes", operation.name, operation.field],
        operation.field === "description" ? operation.value : operation.value.trim(),
      );
      return;
    case "setClassMixins": {
      classOf(model, operation.name);
      const mixins = operation.mixins.map((one) => one.trim()).filter((one) => one !== "");
      for (const mixin of mixins) {
        if (mixin === operation.name) {
          refuse(`class '${operation.name}' cannot mix itself in`);
        }
        classOf(model, mixin);
      }
      // `setOrDelete` reads "" and undefined as empty, not `[]`, so an emptied list is passed
      // as undefined and the key goes rather than staying behind as `mixins: []`.
      setOrDelete(
        document,
        ["classes", operation.name, "mixins"],
        mixins.length > 0 ? mixins : undefined,
      );
      return;
    }
    case "setSlotSubsets": {
      slotOf(model, operation.name);
      const subsets = operation.subsets.map((one) => one.trim()).filter((one) => one !== "");
      for (const subset of subsets) {
        requireName(subset, "subset");
      }
      setOrDelete(
        document,
        ["slots", operation.name, "subsets"],
        subsets.length > 0 ? subsets : undefined,
      );
      return;
    }
    case "addSlot": {
      requireName(operation.name, "slot");
      if (model.slots.some((slot) => slot.name === operation.name)) {
        refuse(`slot '${operation.name}' already exists`);
      }
      const owner = operation.class === undefined ? undefined : classOf(model, operation.class);
      // A node, not a plain object: the kind and the IRI below are set into it in this same
      // operation, and `setIn` can only descend into a YAML collection.
      document.setIn(
        ["slots", operation.name],
        document.createNode({ range: operation.range ?? "string" }),
      );
      if (owner) {
        document.setIn(
          ["classes", owner.name, "slots"],
          [...classSlots(document, owner.name), operation.name],
        );
      }
      // The IRI and the kind go through the same checks a later edit would.
      const declared = parseModel(document.toString());
      const slotUri = operation.slot_uri ?? mintIri(model, operation.name);
      if (slotUri !== undefined) {
        setSlotField(document, declared, operation.name, "slot_uri", slotUri);
      }
      if (operation.kind !== undefined) {
        setSlotField(document, declared, operation.name, "kind", operation.kind);
      }
      return;
    }
    case "removeSlot":
      slotOf(model, operation.name);
      if (relationshipOf(model, operation.name) !== undefined) {
        refuse(`slot '${operation.name}' is an end of a relationship; removeRelationship removes both ends`);
      }
      document.deleteIn(["slots", operation.name]);
      for (const klass of model.classes) {
        // A narrowing of a slot that is gone narrows nothing.
        if (klass.slot_usage?.[operation.name] !== undefined) {
          document.deleteIn(["classes", klass.name, "slot_usage", operation.name]);
        }
        if (klass.slots.includes(operation.name)) {
          document.setIn(
            ["classes", klass.name, "slots"],
            klass.slots.filter((slot) => slot !== operation.name),
          );
        }
      }
      return;
    case "renameSlot": {
      slotOf(model, operation.name);
      requireName(operation.to, "slot");
      if (model.slots.some((slot) => slot.name === operation.to)) {
        refuse(`slot '${operation.to}' already exists`);
      }
      renameKey(document, "slots", operation.name, operation.to);
      for (const klass of model.classes) {
        // A class's narrowing of the slot keeps following it.
        if (klass.slot_usage?.[operation.name] !== undefined) {
          renameKey(document, ["classes", klass.name, "slot_usage"], operation.name, operation.to);
        }
        if (klass.slots.includes(operation.name)) {
          document.setIn(
            ["classes", klass.name, "slots"],
            klass.slots.map((slot) => (slot === operation.name ? operation.to : slot)),
          );
        }
      }
      // The other end names this one by name (DM-64).
      for (const slot of model.slots) {
        if (slot.inverse === operation.name) {
          document.setIn(["slots", slot.name === operation.name ? operation.to : slot.name, "inverse"], operation.to);
        }
      }
      return;
    }
    case "attachSlot": {
      const owner = classOf(model, operation.class);
      slotOf(model, operation.slot);
      if (!owner.slots.includes(operation.slot)) {
        document.setIn(["classes", owner.name, "slots"], [...owner.slots, operation.slot]);
      }
      return;
    }
    case "detachSlot": {
      const owner = classOf(model, operation.class);
      if (!owner.slots.includes(operation.slot)) {
        refuse(`class '${owner.name}' does not use slot '${operation.slot}'`);
      }
      document.setIn(
        ["classes", owner.name, "slots"],
        owner.slots.filter((slot) => slot !== operation.slot),
      );
      return;
    }
    case "setSlot":
      setSlotField(document, model, operation.name, operation.field, operation.value);
      return;
    case "setTitle": {
      const plural = operation.target === "class" ? "classes" : "slots";
      if (operation.target === "class") {
        classOf(model, operation.name);
      } else {
        slotOf(model, operation.name);
      }
      setOrDelete(document, [plural, operation.name, "title", operation.locale], operation.value);
      return;
    }
    case "addEnum":
      requireName(operation.name, "enum");
      if (model.enums.some((entry) => entry.name === operation.name)) {
        refuse(`enum '${operation.name}' already exists`);
      }
      document.setIn(["enums", operation.name, "permissible_values"], {});
      return;
    case "addEnumValue": {
      const entry = enumOf(model, operation.enum);
      if (entry.permissible_values.some((value) => value.name === operation.value)) {
        refuse(`enum '${entry.name}' already has the value '${operation.value}'`);
      }
      document.setIn(
        ["enums", entry.name, "permissible_values", operation.value],
        operation.description ? { description: operation.description } : {},
      );
      return;
    }
    case "setEnumValue": {
      const path = enumValuePath(model, operation.enum, operation.value);
      // `good:` and `good: {}` both declare a value; only a map can carry a title.
      if (!isMap(document.getIn(path, true))) {
        document.setIn(path, document.createNode({}));
      }
      if (operation.field === "description") {
        setOrDelete(document, [...path, "description"], operation.text.trim());
        return;
      }
      const locale = operation.locale?.trim();
      if (!locale) {
        refuse("a title needs the language it is written in");
      }
      // A title written as one string is the model's one language, read as English (UI-86); a
      // second language turns it into a map rather than overwriting it.
      const existing = document.getIn([...path, "title"]);
      if (typeof existing === "string") {
        document.setIn([...path, "title"], document.createNode({ en: existing }));
      }
      setOrDelete(document, [...path, "title", locale], operation.text.trim());
      const left = document.getIn([...path, "title"], true);
      if (isMap(left) && left.items.length === 0) {
        document.deleteIn([...path, "title"]);
      }
      return;
    }
    case "removeEnumValue":
      document.deleteIn(enumValuePath(model, operation.enum, operation.value));
      return;
    case "renameEnum": {
      enumOf(model, operation.name);
      requireName(operation.to, "enum");
      if (model.enums.some((entry) => entry.name === operation.to)) {
        refuse(`enum '${operation.to}' already exists`);
      }
      renameKey(document, "enums", operation.name, operation.to);
      for (const slot of model.slots) {
        if (slot.range === operation.name) {
          document.setIn(["slots", slot.name, "range"], operation.to);
        }
      }
      return;
    }
    case "renameEnumValue": {
      const [, , , value] = enumValuePath(model, operation.enum, operation.value);
      const to = operation.to.trim();
      if (to === "") {
        refuse("a value needs a name");
      }
      if (enumOf(model, operation.enum).permissible_values.some((candidate) => candidate.name === to)) {
        refuse(`enum '${operation.enum}' already has the value '${to}'`);
      }
      renameKey(document, ["enums", operation.enum, "permissible_values"], value, to);
      return;
    }
    case "addRelationship":
      addRelationship(document, model, operation);
      return;
    case "setCardinality": {
      if (!(CARDINALITIES as readonly string[]).includes(operation.cardinality)) {
        refuse(`'${operation.cardinality}' is not a cardinality: ${CARDINALITIES.join(", ")}`);
      }
      const relationship = relationshipNamed(model, operation.name);
      const flags = flagsOf(operation.cardinality);
      const computed = storedEnd(operation.cardinality) === "source" ? relationship.target : relationship.source;
      if (computed.required) {
        refuse(
          `as ${operation.cardinality}, ${computed.slot} would be computed on read, and it is required; ` +
            `clear required on ${computed.slot} first (DM-65)`,
        );
      }
      setOrDelete(document, ["slots", relationship.source.slot, "multivalued"], flags.source);
      setOrDelete(document, ["slots", relationship.target.slot, "multivalued"], flags.target);
      return;
    }
    case "setOnDelete": {
      if (!(ON_DELETE_RULES as readonly string[]).includes(operation.onDelete)) {
        refuse(`'${operation.onDelete}' is not a delete rule: ${ON_DELETE_RULES.join(", ")}`);
      }
      const relationship = relationshipNamed(model, operation.name);
      document.setIn(["slots", relationship.source.slot, "annotations", "on_delete"], operation.onDelete);
      return;
    }
    case "removeRelationship": {
      const relationship = relationshipNamed(model, operation.name);
      for (const end of [relationship.source, relationship.target]) {
        document.deleteIn(["slots", end.slot]);
        for (const klass of model.classes) {
          if (klass.slots.includes(end.slot)) {
            document.setIn(
              ["classes", klass.name, "slots"],
              classSlots(document, klass.name).filter((slot) => slot !== end.slot),
            );
          }
        }
      }
      return;
    }
    case "addInverse": {
      const slot = slotOf(model, operation.name);
      if (slot.kind !== "Relationship" || slot.range === undefined || !model.classes.some((klass) => klass.name === slot.range)) {
        refuse(`slot '${operation.name}' is not a Relationship pointing at a class of this model`);
      }
      if (slot.inverse !== undefined) {
        refuse(`slot '${operation.name}' already names the inverse '${slot.inverse}'`);
      }
      const owners = model.classes.filter((klass) => klass.slots.includes(operation.name));
      if (owners.length !== 1) {
        refuse(`slot '${operation.name}' is used by ${owners.length} classes; a relationship starts on one`);
      }
      addEnd(document, model, {
        name: operation.inverse,
        owner: slot.range ?? "",
        range: owners[0].name,
        inverse: operation.name,
        multivalued: true,
      });
      document.setIn(["slots", operation.name, "inverse"], operation.inverse);
      document.setIn(["slots", operation.name, "inlined"], false);
      // The slot holding the data today is the source, so its entities do not change (DM-73).
      if (slot.on_delete === undefined) {
        document.setIn(["slots", operation.name, "annotations", "on_delete"], "restrict");
      }
      return;
    }
    default:
      refuse(`unknown operation '${(operation as { op: string }).op}'`);
  }
}

/** One end of a relationship written as a slot of its owner (DM-64). */
function addEnd(
  document: Document,
  model: LinkmlModel,
  end: {
    name: string;
    owner: string;
    range: string;
    inverse: string;
    multivalued: boolean;
    required?: boolean;
    onDelete?: OnDelete;
  },
): void {
  requireName(end.name, "slot");
  if (model.slots.some((slot) => slot.name === end.name) || document.hasIn(["slots", end.name])) {
    refuse(`slot '${end.name}' already exists; a relationship's ends are slots of their own`);
  }
  classOf(model, end.owner);
  // Under the model's own prefix, never one reserved for someone else (DM-16): a model whose
  // default prefix is Smart Data Models' gets no IRI here, and diagnose asks for one.
  const minted = model.default_prefix ? `${model.default_prefix}:${end.name}` : undefined;
  const slotUri = minted && !reservedNamespace(minted, model.prefixes) ? minted : undefined;
  document.setIn(
    ["slots", end.name],
    document.createNode({
      range: end.range,
      ...(end.multivalued ? { multivalued: true } : {}),
      ...(end.required ? { required: true } : {}),
      inverse: end.inverse,
      inlined: false,
      ...(slotUri ? { slot_uri: slotUri } : {}),
      annotations: {
        ngsi_ld_kind: "Relationship",
        ...(end.onDelete ? { on_delete: end.onDelete } : {}),
      },
    }),
  );
  // A node, so the second end of a relationship on the same class reads the first one back.
  document.setIn(["classes", end.owner, "slots"], document.createNode([...classSlots(document, end.owner), end.name]));
}

function addRelationship(
  document: Document,
  model: LinkmlModel,
  operation: Extract<Operation, { op: "addRelationship" }>,
): void {
  if (!(CARDINALITIES as readonly string[]).includes(operation.cardinality)) {
    refuse(`'${operation.cardinality}' is not a cardinality: ${CARDINALITIES.join(", ")}`);
  }
  const onDelete = operation.onDelete ?? "restrict";
  if (!(ON_DELETE_RULES as readonly string[]).includes(onDelete)) {
    refuse(`'${onDelete}' is not a delete rule: ${ON_DELETE_RULES.join(", ")}`);
  }
  classOf(model, operation.from);
  const target = model.classes.find((klass) => klass.name === operation.to);
  if (target === undefined) {
    refuse(
      `unknown class '${operation.to}': a relationship's inverse is written on its target, so the target is a class of this model`,
    );
  }
  if (!operation.inverse || operation.inverse.trim() === "") {
    refuse(`the relationship '${operation.name}' needs an inverse: the slot on ${operation.to} that points back (DM-64)`);
  }
  if (operation.inverse === operation.name) {
    refuse(`'${operation.name}' cannot be its own inverse: the two ends are two slots`);
  }
  const flags = flagsOf(operation.cardinality);
  const stored = storedEnd(operation.cardinality);
  if ((stored === "source" && operation.inverseRequired) || (stored === "target" && operation.required)) {
    const computed = stored === "source" ? operation.inverse : operation.name;
    refuse(`as ${operation.cardinality}, ${computed} is computed on read and cannot be required; require the other end (DM-65)`);
  }
  addEnd(document, model, {
    name: operation.name,
    owner: operation.from,
    range: operation.to,
    inverse: operation.inverse,
    multivalued: flags.source,
    required: operation.required,
    onDelete,
  });
  addEnd(document, model, {
    name: operation.inverse,
    owner: operation.to,
    range: operation.from,
    inverse: operation.name,
    multivalued: flags.target,
    required: operation.inverseRequired,
  });
}

/**
 * Applies the operations in order and returns the new source.
 *
 * All or nothing: when any operation is refused the source that came in is returned unchanged
 * together with every refusal, so a caller never has to work out which half of its list landed.
 * Edits go through the YAML document, so comments, key order and formatting survive (DM-13).
 */
export function applyOperations(source: string, operations: Operation[]): Applied {
  const refused: Refusal[] = [];
  let current = source;
  operations.forEach((operation, index) => {
    const model = parseModel(current);
    if (model === EMPTY_MODEL) {
      refused.push({ index, reason: "the source does not parse as YAML" });
      return;
    }
    try {
      current = edit(current, (document) => mutate(document, model, operation));
    } catch (error) {
      refused.push({
        index,
        reason: error instanceof Error ? error.message : String(error),
      });
    }
  });
  return refused.length > 0 ? { source, refused } : { source: current, refused };
}
