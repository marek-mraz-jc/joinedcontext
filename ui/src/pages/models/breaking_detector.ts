/**
 * What changed between the published model and the one being edited, and what that costs
 * (DM-23, DM-24, DM-25).
 *
 * The classification is the same one CI enforces, so the editor refuses at the keystroke what
 * the pipeline would refuse at the merge: a breaking change may not be saved under the major
 * version it breaks, and a published version is never edited in place.
 */
import type { LinkmlEnum, LinkmlModel, LinkmlSlot } from "./linkml";

export type Severity = "none" | "additive" | "breaking";

export interface ModelChange {
  severity: Exclude<Severity, "none">;
  /** What the change is about: a slot name, a class name, an enum value. */
  subject: string;
  /** The rule, in the words the editor shows. */
  reason: string;
}

/** The lifecycle of the model the change is made to, which decides the lane (DM-24). */
export type Lifecycle = "draft" | "published" | "deprecated" | "retired";

export type Lane = "green" | "yellow" | "red";

const NUMERIC = ["integer", "float", "double", "decimal"];

/** Whether `next` accepts every value `previous` did. A narrower range breaks writers. */
function widensOrKeeps(previous: string | undefined, next: string | undefined): boolean {
  if (previous === next) {
    return true;
  }
  if (!previous || !next) {
    // A range that appears where there was none narrows the slot; one that disappears widens it.
    return previous !== undefined;
  }
  if (previous === "integer" && NUMERIC.includes(next) && next !== "integer") {
    return true;
  }
  return next === "string" && previous !== "string";
}

function slotChanges(previous: LinkmlSlot, next: LinkmlSlot): ModelChange[] {
  const changes: ModelChange[] = [];
  const subject = previous.name;

  // T-1094: a projection takes the slots of the profiles it names, so dropping one takes the
  // slot out of every endpoint that projected by it. Adding a profile only widens.
  for (const subset of previous.subsets ?? []) {
    if (!(next.subsets ?? []).includes(subset)) {
      changes.push({
        severity: "breaking",
        subject,
        reason: `the profile ${subset} no longer takes this slot`,
      });
    }
  }
  for (const subset of next.subsets ?? []) {
    if (!(previous.subsets ?? []).includes(subset)) {
      changes.push({
        severity: "additive",
        subject,
        reason: `the profile ${subset} now takes this slot`,
      });
    }
  }

  if (previous.slot_uri !== next.slot_uri) {
    changes.push({
      severity: "breaking",
      subject,
      reason: `the IRI changed from ${previous.slot_uri ?? "none"} to ${next.slot_uri ?? "none"}, so consumers resolve a different term`,
    });
  }
  if (!widensOrKeeps(previous.range, next.range)) {
    changes.push({
      severity: "breaking",
      subject,
      reason: `the range narrowed from ${previous.range ?? "none"} to ${next.range ?? "none"}`,
    });
  } else if (previous.range !== next.range) {
    changes.push({
      severity: "additive",
      subject,
      reason: `the range widened from ${previous.range ?? "none"} to ${next.range ?? "none"}`,
    });
  }
  if (!previous.required && next.required) {
    changes.push({
      severity: "breaking",
      subject,
      reason: "the slot became required, so every existing writer now sends an invalid payload",
    });
  }
  if (previous.required && !next.required) {
    changes.push({ severity: "additive", subject, reason: "the slot is no longer required" });
  }
  if (previous.multivalued !== next.multivalued) {
    changes.push({
      severity: "breaking",
      subject,
      reason: next.multivalued
        ? "the slot became multivalued, so its JSON shape changed"
        : "the slot stopped being multivalued, so its JSON shape changed",
    });
  }
  if (previous.kind !== next.kind) {
    changes.push({
      severity: "breaking",
      subject,
      reason: `the NGSI-LD kind changed from ${previous.kind} to ${next.kind}`,
    });
  }
  if (!previous.deprecated && next.deprecated) {
    changes.push({
      severity: "additive",
      subject,
      reason: "the slot is marked deprecated, which keeps it valid and warns consumers",
    });
  }
  if ((previous.pattern ?? "") !== (next.pattern ?? "") && next.pattern) {
    changes.push({
      severity: "breaking",
      subject,
      reason: "a pattern was added or tightened, so values that validated before may not",
    });
  }
  if (numberTightened(previous.minimum_value, next.minimum_value, "up")) {
    changes.push({ severity: "breaking", subject, reason: "the minimum value was raised" });
  }
  if (numberTightened(previous.maximum_value, next.maximum_value, "down")) {
    changes.push({ severity: "breaking", subject, reason: "the maximum value was lowered" });
  }
  if ((previous.description ?? "") !== (next.description ?? "")) {
    changes.push({ severity: "additive", subject, reason: "the description changed" });
  }
  return changes;
}

function numberTightened(
  previous: number | undefined,
  next: number | undefined,
  direction: "up" | "down",
): boolean {
  if (next === undefined) {
    return false;
  }
  if (previous === undefined) {
    return true;
  }
  return direction === "up" ? next > previous : next < previous;
}

function enumChanges(previous: LinkmlEnum, next: LinkmlEnum): ModelChange[] {
  const changes: ModelChange[] = [];
  const before = new Set(previous.permissible_values.map((value) => value.name));
  const after = new Set(next.permissible_values.map((value) => value.name));
  for (const value of before) {
    if (!after.has(value)) {
      changes.push({
        severity: "breaking",
        subject: `${previous.name}.${value}`,
        reason: "an enum value was removed, so stored entities carry a value the model rejects",
      });
    }
  }
  for (const value of after) {
    if (!before.has(value)) {
      changes.push({
        severity: "additive",
        subject: `${previous.name}.${value}`,
        reason: "a new enum value was added",
      });
    }
  }
  return changes;
}

/** Every difference between the published model and the edited one, classified (DM-23). */
export function classifyChanges(previous: LinkmlModel, next: LinkmlModel): ModelChange[] {
  // T-1094: an import brings terms this model then uses; dropping one can take a range or a
  // type out from under a slot that still names it, which a reader of the old model relied on.
  const changes: ModelChange[] = [];

  for (const imported of previous.imports ?? []) {
    if (!(next.imports ?? []).includes(imported)) {
      changes.push({
        severity: "breaking",
        subject: previous.name ?? "the model",
        reason: `the import ${imported} was removed, with whatever it brought`,
      });
    }
  }
  for (const imported of next.imports ?? []) {
    if (!(previous.imports ?? []).includes(imported)) {
      changes.push({
        severity: "additive",
        subject: next.name ?? "the model",
        reason: `the import ${imported} was added`,
      });
    }
  }
  const previousClasses = new Map(previous.classes.map((klass) => [klass.name, klass]));
  const nextClasses = new Map(next.classes.map((klass) => [klass.name, klass]));
  for (const [name, klass] of previousClasses) {
    const now = nextClasses.get(name);
    if (!now) {
      changes.push({
        severity: "breaking",
        subject: name,
        reason: "the class was removed, and its entity type stops validating",
      });
      continue;
    }
    if (klass.class_uri !== now.class_uri) {
      changes.push({
        severity: "breaking",
        subject: name,
        reason: `the class IRI changed from ${klass.class_uri ?? "none"} to ${now.class_uri ?? "none"}`,
      });
    }
    // T-1094, DM-13: a class inherits its parent's and its mixins' slots, so losing one takes
    // away everything it brought — a reader of the old model may hold ids and attributes the
    // new one no longer promises. Gaining one only adds.
    if ((klass.is_a ?? "") !== (now.is_a ?? "")) {
      changes.push({
        severity: klass.is_a === undefined ? "additive" : "breaking",
        subject: name,
        reason: `the class it specialises changed from ${klass.is_a ?? "none"} to ${
          now.is_a ?? "none"
        }`,
      });
    }
    for (const mixin of klass.mixins ?? []) {
      if (!(now.mixins ?? []).includes(mixin)) {
        changes.push({
          severity: "breaking",
          subject: name,
          reason: `the mixin ${mixin} was removed, with whatever it brought`,
        });
      }
    }
    for (const mixin of now.mixins ?? []) {
      if (!(klass.mixins ?? []).includes(mixin)) {
        changes.push({
          severity: "additive",
          subject: name,
          reason: `the mixin ${mixin} was added`,
        });
      }
    }
    for (const slot of klass.slots) {
      if (!now.slots.includes(slot)) {
        changes.push({
          severity: "breaking",
          subject: `${name}.${slot}`,
          reason: "the slot was removed from the class",
        });
      }
    }
  }
  for (const name of nextClasses.keys()) {
    if (!previousClasses.has(name)) {
      changes.push({ severity: "additive", subject: name, reason: "a new class was added" });
    }
  }

  const previousSlots = new Map(previous.slots.map((slot) => [slot.name, slot]));
  const nextSlots = new Map(next.slots.map((slot) => [slot.name, slot]));
  for (const [name, slot] of previousSlots) {
    const now = nextSlots.get(name);
    if (!now) {
      changes.push({
        severity: "breaking",
        subject: name,
        reason: "the slot was removed; a renamed slot is a removal and an addition",
      });
      continue;
    }
    changes.push(...slotChanges(slot, now));
  }
  for (const [name, slot] of nextSlots) {
    if (!previousSlots.has(name)) {
      changes.push({
        severity: slot.required ? "breaking" : "additive",
        subject: name,
        reason: slot.required
          ? "a new required slot rejects every payload written before it existed"
          : "a new optional slot was added",
      });
    }
  }

  const previousEnums = new Map(previous.enums.map((entry) => [entry.name, entry]));
  const nextEnums = new Map(next.enums.map((entry) => [entry.name, entry]));
  for (const [name, entry] of previousEnums) {
    const now = nextEnums.get(name);
    if (!now) {
      changes.push({ severity: "breaking", subject: name, reason: "the enum was removed" });
      continue;
    }
    changes.push(...enumChanges(entry, now));
  }
  for (const [name] of nextEnums) {
    if (!previousEnums.has(name)) {
      changes.push({ severity: "additive", subject: name, reason: "a new enum was added" });
    }
  }

  return changes;
}

/** The worst severity in the set: one breaking change makes the whole edit breaking. */
export function severityOf(changes: ModelChange[]): Severity {
  if (changes.some((change) => change.severity === "breaking")) {
    return "breaking";
  }
  return changes.length > 0 ? "additive" : "none";
}

/**
 * The lane this edit belongs in (DM-24, CC-63).
 *
 * A draft model is the author's own; an additive change to a published one needs a domain
 * approver; a breaking change, or any change to a model a public endpoint serves, is red.
 */
export function laneOf(
  changes: ModelChange[],
  lifecycle: Lifecycle,
  servedByPublicEndpoint = false,
): Lane {
  if (severityOf(changes) === "breaking" || servedByPublicEndpoint) {
    return "red";
  }
  if (lifecycle === "draft") {
    return "green";
  }
  return severityOf(changes) === "none" ? "green" : "yellow";
}

/** The version this edit has to carry, from the published one (DM-23). */
export function bumpVersion(version: string, changes: ModelChange[]): string {
  const [major = 0, minor = 0, patch = 0] = version
    .split(".")
    .map((part) => Number.parseInt(part, 10) || 0);
  switch (severityOf(changes)) {
    case "breaking":
      return `${major + 1}.0.0`;
    case "additive":
      return `${major}.${minor + 1}.0`;
    default:
      return `${major}.${minor}.${patch}`;
  }
}

/**
 * The version a proposal of this edit becomes, as the API bumps it (DM-22, DM-23): breaking the
 * major, additive the minor, any other edit (a comment, a reordering) the patch. An unedited model
 * keeps its version: there is nothing to propose.
 */
export function proposedVersion(version: string, changes: ModelChange[], edited: boolean): string {
  if (!edited) return version;
  if (severityOf(changes) !== "none") return bumpVersion(version, changes);
  const [major = 0, minor = 0, patch = 0] = version.split(".").map((part) => Number.parseInt(part, 10) || 0);
  return `${major}.${minor}.${patch + 1}`;
}

/** What an edit does to the model's fields, classes and enums, counted the way a reviewer reads a diff. */
export type ChangeAction = "added" | "changed" | "removed";

export function actionOf(change: ModelChange): ChangeAction {
  if (/^a new \b|was added$|\bnow takes\b/.test(change.reason)) return "added";
  if (/\bwas removed\b|\bno longer takes\b/.test(change.reason)) return "removed";
  return "changed";
}

export function diffCounts(changes: ModelChange[]): Record<ChangeAction, number> {
  const counts: Record<ChangeAction, number> = { added: 0, changed: 0, removed: 0 };
  for (const change of changes) counts[actionOf(change)] += 1;
  return counts;
}

/**
 * Why this edit may not be saved under the version it names, or `undefined` when it may.
 *
 * The editor refuses rather than warns: a breaking change saved under the same major is a
 * silent break for every consumer that pinned it (DM-23, SP-13).
 */
export function refusalToSave(
  changes: ModelChange[],
  publishedVersion: string,
  proposedVersion: string,
): string | undefined {
  const majorOf = (version: string) => Number.parseInt(version.split(".")[0] ?? "0", 10) || 0;
  if (severityOf(changes) !== "breaking") {
    return undefined;
  }
  if (majorOf(proposedVersion) > majorOf(publishedVersion)) {
    return undefined;
  }
  return `a breaking change cannot be saved under version ${proposedVersion}; publish it as ${bumpVersion(publishedVersion, changes)}`;
}
