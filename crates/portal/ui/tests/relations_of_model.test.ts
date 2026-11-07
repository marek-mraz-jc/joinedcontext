/** T-2741: the grid's relationship ends come from the space's model, stored and computed (UI-84, DM-64, DM-67). */
import { describe, expect, it } from "vitest";
import { relationsOfModel } from "../src/components/entities/filters";

const MODEL = `id: https://bb.sk/models/skoly
name: skoly
prefixes:
  bb: https://bb.sk/terms/
default_prefix: bb
classes:
  School:
    slots: [name, users]
  User:
    slots: [name, school, courses]
  Course:
    slots: [name, attendees]
slots:
  name:
    range: string
  users:
    range: User
    multivalued: true
    inverse: school
    annotations:
      ngsi_ld_kind: Relationship
      on_delete: restrict
  school:
    range: School
    required: true
    inverse: users
    annotations:
      ngsi_ld_kind: Relationship
  courses:
    range: Course
    multivalued: true
    inverse: attendees
    annotations:
      ngsi_ld_kind: Relationship
      on_delete: restrict
  attendees:
    range: User
    multivalued: true
    inverse: courses
    annotations:
      ngsi_ld_kind: Relationship
enums: {}
`;

describe("relationsOfModel", () => {
  it("gives a class its stored ends as pickers, with their cardinality and requirement", () => {
    expect(relationsOfModel(MODEL, "User")).toEqual({
      school: { target: "School", many: false, required: true },
      courses: { target: "Course", many: true, required: false },
    });
  });

  it("gives the other class the computed end, read from the stored slot and never required", () => {
    expect(relationsOfModel(MODEL, "School")).toEqual({
      users: { target: "User", many: true, required: false, inverseOf: "school" },
    });
    expect(relationsOfModel(MODEL, "Course")).toEqual({
      attendees: { target: "User", many: true, required: false, inverseOf: "courses" },
    });
  });

  it("has nothing without a model or a type", () => {
    expect(relationsOfModel(undefined, "User")).toEqual({});
    expect(relationsOfModel(MODEL, undefined)).toEqual({});
    expect(relationsOfModel(MODEL, "Unknown")).toEqual({});
  });
});
