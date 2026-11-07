/** T-2736: relationships strict like foreign keys, in the operations and in diagnose (DM-64…DM-69, DM-73). */
import { describe, expect, it } from "vitest";
import { diagnose, parseModel, relationships, relationshipsOf } from "../src/pages/models/linkml";
import type { Cardinality, RelationshipRule } from "../src/pages/models/linkml";
import { applyOperations } from "../src/pages/models/operations";
import type { Operation } from "../src/pages/models/operations";

const SCHOOL = `# rozvoj: schools, their users and courses
id: https://rozvoj.sk/models/school
name: school
prefixes:
  rozvoj: https://rozvoj.sk/terms/
default_prefix: rozvoj
classes:
  School:
    class_uri: rozvoj:School
    slots:
      - name
  User:
    class_uri: rozvoj:User
    slots: []
slots:
  name:
    range: string
    slot_uri: rozvoj:name
enums: {}
`;

function applied(source: string, operations: Operation[]): string {
  const result = applyOperations(source, operations);
  expect(result.refused).toEqual([]);
  return result.source;
}

function refusals(source: string, operations: Operation[]): string[] {
  const result = applyOperations(source, operations);
  expect(result.source).toBe(source);
  return result.refused.map((refusal) => refusal.reason);
}

function rulesOf(source: string): RelationshipRule[] {
  return diagnose(source).flatMap((diagnostic) => (diagnostic.rule ? [diagnostic.rule] : []));
}

const add = (cardinality: Cardinality, extra: Partial<Extract<Operation, { op: "addRelationship" }>> = {}): Operation => ({
  op: "addRelationship",
  from: "School",
  to: "User",
  name: "users",
  inverse: "school",
  cardinality,
  ...extra,
});

describe("addRelationship writes both ends", () => {
  it("one-to-many: School.users many, User.school one and stored, as golden YAML", () => {
    const source = applied(SCHOOL, [add("one-to-many", { inverseRequired: true })]);
    expect(source).toContain(`  users:
    range: User
    multivalued: true
    inverse: school
    inlined: false
    slot_uri: rozvoj:users
    annotations:
      ngsi_ld_kind: Relationship
      on_delete: restrict
  school:
    range: School
    required: true
    inverse: users
    inlined: false
    slot_uri: rozvoj:school
    annotations:
      ngsi_ld_kind: Relationship
`);
    expect(parseModel(source).classes.map((klass) => [klass.name, klass.slots])).toEqual([
      ["School", ["name", "users"]],
      ["User", ["school"]],
    ]);
    expect(relationships(parseModel(source))).toEqual([
      {
        source: { class: "School", slot: "users", multivalued: true, required: false },
        target: { class: "User", slot: "school", multivalued: false, required: true },
        cardinality: "one-to-many",
        onDelete: "restrict",
        stored: "target",
      },
    ]);
    expect(diagnose(source).filter((d) => d.severity === "error")).toEqual([]);
    expect(source).toContain("# rozvoj: schools, their users and courses");
  });

  it("many-to-one: the source holds the id", () => {
    const source = applied(SCHOOL, [
      { op: "addRelationship", from: "User", to: "School", name: "school", inverse: "users", cardinality: "many-to-one", required: true, onDelete: "cascade" },
    ]);
    expect(source).toContain(`  school:
    range: School
    required: true
    inverse: users
    inlined: false
    slot_uri: rozvoj:school
    annotations:
      ngsi_ld_kind: Relationship
      on_delete: cascade
  users:
    range: User
    multivalued: true
    inverse: school
`);
    expect(relationships(parseModel(source))[0]).toMatchObject({ cardinality: "many-to-one", stored: "source", onDelete: "cascade" });
  });

  it("one-to-one: neither end is multivalued and the source is stored", () => {
    const source = applied(SCHOOL, [add("one-to-one", { name: "principal", inverse: "leads", required: true })]);
    const [relationship] = relationships(parseModel(source));
    expect(relationship).toMatchObject({ cardinality: "one-to-one", stored: "source" });
    expect(source).not.toContain("multivalued");
    expect(source).toContain(`  principal:
    range: User
    required: true
    inverse: leads
`);
  });

  it("many-to-many: both ends multivalued, stored on the source", () => {
    const source = applied(SCHOOL, [add("many-to-many", { onDelete: "set-null" })]);
    expect(relationships(parseModel(source))[0]).toMatchObject({ cardinality: "many-to-many", stored: "source", onDelete: "set-null" });
    expect(source.match(/multivalued: true/g)).toHaveLength(2);
  });

  it("mints the ends' IRIs under the model's own prefix, never under a reserved one (DM-16)", () => {
    const sdm = SCHOOL.replace("  rozvoj: https://rozvoj.sk/terms/\n", "  rozvoj: https://rozvoj.sk/terms/\n  sdm: https://smartdatamodels.org/\n").replace(
      "default_prefix: rozvoj",
      "default_prefix: sdm",
    );
    const source = applied(sdm, [add("one-to-many")]);
    expect(source).not.toContain("sdm:users");
    expect(parseModel(source).slots.find((slot) => slot.name === "users")?.slot_uri).toBeUndefined();
  });

  it("a class relates to itself: Person.manager N:1 Person, inverse reports", () => {
    const people = SCHOOL.replace("  User:\n", "  Person:\n    slots: []\n  User:\n");
    const source = applied(people, [
      { op: "addRelationship", from: "Person", to: "Person", name: "manager", inverse: "reports", cardinality: "many-to-one" },
    ]);
    expect(parseModel(source).classes.find((klass) => klass.name === "Person")?.slots).toEqual(["manager", "reports"]);
    expect(relationships(parseModel(source))).toEqual([
      {
        source: { class: "Person", slot: "manager", multivalued: false, required: false },
        target: { class: "Person", slot: "reports", multivalued: true, required: false },
        cardinality: "many-to-one",
        onDelete: "restrict",
        stored: "source",
      },
    ]);
    expect(rulesOf(source)).toEqual([]);
  });
});

describe("addRelationship refuses", () => {
  it("a missing inverse, a duplicate name, an unknown class, a bad cardinality and more", () => {
    expect(refusals(SCHOOL, [add("one-to-many", { inverse: "" })])[0]).toMatch(/needs an inverse/);
    expect(refusals(SCHOOL, [add("one-to-many", { name: "name" })])[0]).toMatch(/slot 'name' already exists/);
    expect(refusals(SCHOOL, [add("one-to-many", { to: "Course" })])[0]).toMatch(/unknown class 'Course'/);
    expect(refusals(SCHOOL, [add("one-to-many", { from: "Course" })])[0]).toMatch(/unknown class 'Course'/);
    expect(refusals(SCHOOL, [add("few-to-some" as Cardinality)])[0]).toMatch(/not a cardinality/);
    expect(refusals(SCHOOL, [add("one-to-many", { inverse: "users" })])[0]).toMatch(/cannot be its own inverse/);
    expect(refusals(SCHOOL, [add("one-to-many", { name: "bad name" })])[0]).toMatch(/not a valid slot name/);
    expect(refusals(SCHOOL, [add("one-to-many", { onDelete: "vanish" as never })])[0]).toMatch(/not a delete rule/);
    // The computed end cannot be required (DM-65): for 1:N the source is the query.
    expect(refusals(SCHOOL, [add("one-to-many", { required: true })])[0]).toMatch(/users is computed on read/);
    expect(refusals(SCHOOL, [add("many-to-one", { inverseRequired: true })])[0]).toMatch(/school is computed on read/);
  });
});

describe("the ends change together", () => {
  const withUsers = () => applied(SCHOOL, [add("one-to-many")]);

  it("setCardinality flips both flags and keeps required off the new computed end", () => {
    const source = applied(withUsers(), [{ op: "setCardinality", name: "school", cardinality: "many-to-many" }]);
    expect(relationships(parseModel(source))[0]).toMatchObject({ cardinality: "many-to-many", stored: "source" });
    const back = applied(source, [{ op: "setCardinality", name: "users", cardinality: "one-to-one" }]);
    expect(back).not.toContain("multivalued");

    const required = applied(SCHOOL, [add("one-to-many", { inverseRequired: true })]);
    expect(refusals(required, [{ op: "setCardinality", name: "users", cardinality: "many-to-one" }])[0]).toMatch(
      /school would be computed on read, and it is required/,
    );
  });

  it("setOnDelete writes the rule on the source end, whichever end is named", () => {
    const source = applied(withUsers(), [{ op: "setOnDelete", name: "school", onDelete: "cascade" }]);
    expect(parseModel(source).slots.find((slot) => slot.name === "users")?.on_delete).toBe("cascade");
    expect(parseModel(source).slots.find((slot) => slot.name === "school")?.on_delete).toBeUndefined();
    expect(refusals(withUsers(), [{ op: "setOnDelete", name: "name", onDelete: "cascade" }])[0]).toMatch(/not an end of a relationship/);
  });

  it("renaming a class or a slot keeps both ends pointing at each other", () => {
    const source = applied(withUsers(), [
      { op: "renameClass", name: "School", to: "Academy" },
      { op: "renameSlot", name: "school", to: "academy" },
    ]);
    expect(relationships(parseModel(source))).toEqual([
      expect.objectContaining({
        source: expect.objectContaining({ class: "Academy", slot: "users" }),
        target: expect.objectContaining({ class: "User", slot: "academy" }),
      }),
    ]);
    expect(rulesOf(source)).toEqual([]);
  });

  it("removing one end, or a class an end sits on, is refused; removeRelationship removes both", () => {
    expect(refusals(withUsers(), [{ op: "removeSlot", name: "school" }])[0]).toMatch(/removeRelationship removes both ends/);
    expect(refusals(withUsers(), [{ op: "removeClass", name: "User" }])[0]).toMatch(/remove those relationships first/);
    expect(refusals(withUsers(), [{ op: "setSlot", name: "users", field: "multivalued", value: false }])[0]).toMatch(/setCardinality/);
    expect(refusals(withUsers(), [{ op: "setSlot", name: "users", field: "range", value: "string" }])[0]).toMatch(/removeRelationship/);
    expect(refusals(withUsers(), [{ op: "setSlot", name: "users", field: "required", value: true }])[0]).toMatch(/cannot be required/);

    const source = applied(withUsers(), [{ op: "removeRelationship", name: "school" }]);
    expect(source).toBe(SCHOOL.replace("    slots: []\n", "    slots: []\n"));
    expect(parseModel(source).slots.map((slot) => slot.name)).toEqual(["name"]);
  });

  it("addInverse fixes a relationship saved without one and leaves the stored end where it is (DM-73)", () => {
    const legacy = SCHOOL.replace("    slots: []\n", "    slots:\n      - school\n").replace(
      "enums: {}\n",
      "  school:\n    range: School\n    slot_uri: rozvoj:school\n    annotations:\n      ngsi_ld_kind: Relationship\nenums: {}\n",
    );
    expect(rulesOf(legacy)).toEqual(["inverse-missing"]);
    const fixed = applied(legacy, [{ op: "addInverse", name: "school", inverse: "users" }]);
    expect(rulesOf(fixed)).toEqual([]);
    expect(relationships(parseModel(fixed))[0]).toMatchObject({
      source: { class: "User", slot: "school" },
      cardinality: "many-to-one",
      stored: "source",
    });
    expect(refusals(fixed, [{ op: "addInverse", name: "school", inverse: "members" }])[0]).toMatch(/already names the inverse/);
    expect(refusals(fixed, [{ op: "addInverse", name: "name", inverse: "x" }])[0]).toMatch(/not a Relationship/);
  });
});

/** A model with one slot pair written by hand, for the rules the operations never write. */
function model(slots: string, classes = "  School:\n    slots: [users]\n  User:\n    slots: [school]\n"): string {
  return `id: https://rozvoj.sk/models/school\nname: school\nclasses:\n${classes}slots:\n${slots}`;
}

const USERS = "  users:\n    range: User\n    multivalued: true\n    inverse: school\n    annotations:\n      ngsi_ld_kind: Relationship\n";
const SCHOOL_END = "  school:\n    range: School\n    inverse: users\n    annotations:\n      ngsi_ld_kind: Relationship\n";

describe("diagnose refuses each rule of DM-68 as an error with its path", () => {
  it("the well-formed pair is clean", () => {
    expect(rulesOf(model(USERS + SCHOOL_END))).toEqual([]);
  });

  it.each<[RelationshipRule, string, string?]>([
    ["range-not-a-class", USERS.replace("range: User", "range: Teacher") + SCHOOL_END],
    ["class-range-not-relationship", USERS + SCHOOL_END.replace("ngsi_ld_kind: Relationship", "ngsi_ld_kind: Property")],
    ["primitive-range", USERS.replace("range: User", "range: string") + SCHOOL_END],
    ["inverse-missing", USERS.replace("    inverse: school\n", "") + SCHOOL_END],
    ["inverse-missing", USERS.replace("inverse: school", "inverse: campus") + SCHOOL_END],
    ["inverse-not-reciprocal", USERS + SCHOOL_END.replace("inverse: users", "inverse: pupils")],
    [
      "slot-in-two-relationships",
      USERS + SCHOOL_END,
      "  School:\n    slots: [users]\n  User:\n    slots: [school]\n  Club:\n    slots: [users]\n",
    ],
    ["required-on-computed-end", USERS.replace("multivalued: true", "multivalued: true\n    required: true") + SCHOOL_END],
    ["on-delete-unknown", USERS.replace("ngsi_ld_kind: Relationship", "ngsi_ld_kind: Relationship\n      on_delete: vanish") + SCHOOL_END],
    [
      "on-delete-on-both-ends",
      USERS.replace("ngsi_ld_kind: Relationship", "ngsi_ld_kind: Relationship\n      on_delete: cascade") +
        SCHOOL_END.replace("ngsi_ld_kind: Relationship", "ngsi_ld_kind: Relationship\n      on_delete: restrict"),
    ],
  ])("%s", (rule, slots, classes) => {
    const source = model(slots, classes);
    const found = diagnose(source).filter((diagnostic) => diagnostic.rule === rule);
    expect(found.length).toBeGreaterThan(0);
    for (const diagnostic of found) {
      expect(diagnostic.severity).toBe("error");
      expect(diagnostic.path).toMatch(/^slots\.(users|school)$/);
      expect(diagnostic.line).toBeGreaterThan(1);
    }
  });

  it("a nested value may take a class range: a JsonProperty, or a slot inlined: true (the importer's address)", () => {
    const nested = "  address:\n    range: Address\n    annotations:\n      ngsi_ld_kind: JsonProperty\n  contact:\n    range: Address\n    inlined: true\n";
    expect(rulesOf(model(nested, "  School:\n    slots: [address, contact]\n  Address:\n    slots: []\n"))).toEqual([]);
    const plain = "  seat:\n    range: Address\n";
    expect(rulesOf(model(plain, "  School:\n    slots: [seat]\n  Address:\n    slots: []\n"))).toEqual(["class-range-not-relationship"]);
  });

  it("an external reference (uriorcurie, or no range at all) needs no inverse; one that names an inverse is refused", () => {
    const external = "  refDevice:\n    annotations:\n      ngsi_ld_kind: Relationship\n  derivedFrom:\n    range: uriorcurie\n    annotations:\n      ngsi_ld_kind: Relationship\n";
    expect(rulesOf(model(external, "  School:\n    slots: [refDevice, derivedFrom]\n"))).toEqual([]);
    const named = external.replace("    range: uriorcurie\n", "    range: uriorcurie\n    inverse: users\n");
    expect(rulesOf(model(named, "  School:\n    slots: [refDevice, derivedFrom]\n"))).toEqual(["range-not-a-class"]);
  });

  it("a range the loaded model cannot see is a warning while an import is not in hand, and resolves once it is", () => {
    const importing = model(
      "  district:\n    range: CityDistrict\n    inverse: schools\n    annotations:\n      ngsi_ld_kind: Relationship\n",
      "  School:\n    slots: [district]\n",
    ).replace("classes:", "imports:\n  - linkml:types\n  - ngsi-ld-core\n  - ./districts.linkml.yaml\nclasses:");
    const unseen = diagnose(importing).filter(
      (diagnostic) => diagnostic.path === "slots.district" && /imports it may come from/.test(diagnostic.message),
    );
    expect(unseen.map((diagnostic) => diagnostic.severity)).toEqual(["warning"]);

    const districts = parseModel(
      "id: https://rozvoj.sk/models/districts\nname: districts\nclasses:\n  CityDistrict:\n    slots: [schools]\nslots:\n  schools:\n    range: School\n    multivalued: true\n    inverse: district\n    annotations:\n      ngsi_ld_kind: Relationship\n",
    );
    const found = relationshipsOf(parseModel(importing), { districts });
    expect(found.problems).toEqual([]);
    expect(found.relationships[0]).toMatchObject({ cardinality: "many-to-one", target: { class: "CityDistrict", slot: "schools" } });

    const withoutInverse = parseModel("id: x\nname: districts\nclasses:\n  CityDistrict:\n    slots: []\n");
    expect(relationshipsOf(parseModel(importing), { districts: withoutInverse }).problems).toEqual([
      expect.objectContaining({ rule: "inverse-missing", message: expect.stringMatching(/from districts does not have/) }),
    ]);
  });
});
