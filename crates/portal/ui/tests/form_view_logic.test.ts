/**
 * T-3103, API/01 §30: a form view's fields come from the type's LinkML class and the view's
 * settings; conditions hide fields, the URL prefills only what the form asks for, a submission is
 * one NGSI-LD entity and nothing blank or hidden is sent.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/formView.ts.
import { describe, expect, it } from "vitest";
import { classSlots, parseModel } from "../src/pages/models/linkml";
import type { LinkmlSlot } from "../src/pages/models/linkml";
import { enumsOfModel } from "../src/components/entities/filters";
import { createdId, entityOf, fieldsOf, fieldsOfSchema, prefilled, problemsOf, trapName, visibleFields } from "../src/pages/spaces/formView";
import type { FormField } from "../src/pages/spaces/formView";

const slot = (name: string, more: Partial<LinkmlSlot> = {}): LinkmlSlot => ({ name, kind: "Property", ...more });
const SLOTS: LinkmlSlot[] = [
  slot("code", { identifier: true }),
  slot("name", { kind: "LanguageProperty", required: true, title: { sk: "Názov", en: "Name" } }),
  slot("category", { range: "ParkingCategory", description: "What kind of place" }),
  slot("capacity", { range: "integer" }),
  slot("fee", { range: "float" }),
  slot("covered", { range: "boolean" }),
  slot("opened", { range: "date" }),
  slot("refOperator", { kind: "Relationship" }),
  slot("location", { kind: "GeoProperty" }),
  slot("tags", { multivalued: true }),
  slot("old", { deprecated: true }),
  slot("raw", { kind: "JsonProperty" }),
];
const ENUMS = { category: [{ value: "street" }, { value: "garage", title: "Garáž" }] };

describe("the fields", () => {
  it("are the askable slots in the model's order, labelled and typed from the model", () => {
    const fields = fieldsOf(SLOTS, ENUMS, undefined, "sk");
    expect(fields.map((f) => [f.attr, f.kind])).toEqual([
      ["name", "language"],
      ["category", "enum"],
      ["capacity", "integer"],
      ["fee", "number"],
      ["covered", "boolean"],
      ["opened", "date"],
      ["refOperator", "relationship"],
      ["location", "point"],
    ]);
    expect(fields[0]).toMatchObject({ label: "Názov", required: true });
    expect(fields[1]).toMatchObject({ label: "category", help: "What kind of place", options: ENUMS.category });
  });

  it("follow the settings' order and labels, and settings can add a requirement but not lift one", () => {
    const fields = fieldsOf(
      SLOTS,
      ENUMS,
      { fields: [{ attr: "capacity", label: "Places", required: true }, { attr: "name", required: false }, { attr: "nope" }, { attr: "code" }] },
      "en",
    );
    expect(fields.map((f) => [f.attr, f.label, f.required])).toEqual([
      ["capacity", "Places", true],
      ["name", "Name", true],
    ]);
  });
});

describe("the labels a seed gives", () => {
  // The seeds' shape (T-5093): the slot has no title, the class labels it in slot_usage, and each
  // enum value carries its label as an annotation.
  const SEED = `
id: https://example.org/m
name: m
slots:
  startDate: { range: datetime }
  eventStatus: { range: EventStatus }
classes:
  Event:
    slots: [startDate, eventStatus]
    slot_usage:
      startDate:
        annotations:
          label: { tag: label, value: { fi: Alkaa, en: Starts, sk: Začiatok } }
      eventStatus:
        annotations:
          label: { tag: label, value: { en: Status, sk: Stav } }
enums:
  EventStatus:
    permissible_values:
      EventScheduled:
        annotations:
          label: { tag: label, value: { en: Scheduled, sk: Naplánované } }
      EventCancelled: {}
`;

  it("label the form's fields and options by the model's labels, never the raw attribute", () => {
    const model = parseModel(SEED);
    const slots = classSlots(model, model.classes[0]);
    const fields = fieldsOf(slots, enumsOfModel(SEED, "Event", "sk"), undefined, "sk");
    expect(fields.map((f) => [f.attr, f.label, f.kind])).toEqual([
      ["startDate", "Začiatok", "datetime"],
      ["eventStatus", "Stav", "enum"],
    ]);
    expect(fields[1].options).toEqual([
      { value: "EventScheduled", title: "Naplánované", description: undefined },
      { value: "EventCancelled", title: undefined, description: undefined },
    ]);
    // A language the model does not label falls back to English, then to the attribute.
    expect(fieldsOf(slots, {}, undefined, "de").map((f) => f.label)).toEqual(["Starts", "Status"]);
  });
});

describe("conditions and prefill", () => {
  const fields = fieldsOf(SLOTS, ENUMS, undefined, "en");
  const conditions = [{ attr: "capacity", when: { attr: "category", equals: "garage" } }];

  it("show a field only while the other field holds the value", () => {
    expect(visibleFields(fields, conditions, {}).map((f) => f.attr)).not.toContain("capacity");
    expect(visibleFields(fields, conditions, { category: "street" }).map((f) => f.attr)).not.toContain("capacity");
    expect(visibleFields(fields, conditions, { category: "garage" }).map((f) => f.attr)).toContain("capacity");
  });

  it("prefill only the fields the form asks for, an enum only with one of its values", () => {
    const search = new URLSearchParams("name=Hlavná&category=moon&capacity=12&code=X&location.lat=48.7&location.lon=19.1");
    expect(prefilled(fields, search, undefined)).toEqual({
      name: "Hlavná",
      capacity: "12",
      "location.lat": "48.7",
      "location.lon": "19.1",
    });
    expect(prefilled(fields, search, { prefill: false })).toEqual({});
  });
});

describe("what is sent", () => {
  const fields = fieldsOf(SLOTS, ENUMS, undefined, "sk");

  it("names every answer it cannot send, and a required blank", () => {
    expect(
      problemsOf(fields, {
        capacity: "1.5",
        fee: "lots",
        refOperator: "operator-7",
        opened: "7.10.2026",
        "location.lon": "19",
      }),
    ).toEqual({ name: "required", capacity: "integer", fee: "number", refOperator: "urn", opened: "date", location: "point" });
    expect(problemsOf(fields, { name: "A" })).toEqual({});
  });

  it("is one NGSI-LD entity of the answered fields, typed, with nothing blank or hidden in it", () => {
    const shown = visibleFields(fields, [{ attr: "fee", when: { attr: "category", equals: "garage" } }], { category: "street" });
    const entity = entityOf(
      "ParkingSpot",
      shown,
      {
        name: " Hlavná ",
        category: "street",
        capacity: "12",
        fee: "2.5",
        covered: "true",
        opened: "2026-10-07",
        refOperator: "urn:ngsi-ld:Organization:city",
        "location.lon": "19.15",
        "location.lat": "48.74",
      },
      "sk",
      "urn:ngsi-ld:ParkingSpot:1",
    );
    expect(entity).toEqual({
      id: "urn:ngsi-ld:ParkingSpot:1",
      type: "ParkingSpot",
      name: { type: "LanguageProperty", languageMap: { sk: "Hlavná" } },
      category: { type: "Property", value: "street" },
      capacity: { type: "Property", value: 12 },
      covered: { type: "Property", value: true },
      opened: { type: "Property", value: { "@type": "Date", "@value": "2026-10-07" } },
      refOperator: { type: "Relationship", object: "urn:ngsi-ld:Organization:city" },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [19.15, 48.74] } },
    });
    expect(String(entityOf("ParkingSpot", [], {}, "sk").id)).toMatch(/^urn:ngsi-ld:ParkingSpot:[0-9a-f-]{36}$/);
  });
});

describe("a public form's fields", () => {
  it("are the published schema's attributes of the type, typed and required as the schema says", () => {
    const definition = {
      required: ["name", "id", "type"],
      properties: {
        id: { type: "string" },
        type: { type: "string" },
        name: { type: ["string", "null"], "x-ngsi-ld-kind": "Property", description: "What it is called" },
        capacity: { type: ["integer", "null"], "x-ngsi-ld-kind": "Property" },
        category: { $ref: "#/definitions/ParkingCategory", "x-ngsi-ld-kind": "Property" },
        opened: { type: ["string", "null"], format: "date-time" },
        refOperator: { type: ["string", "null"], "x-ngsi-ld-kind": "Relationship" },
        location: { "x-ngsi-ld-kind": "GeoProperty", properties: {} },
        tags: { type: ["array", "null"] },
        raw: { "x-ngsi-ld-kind": "JsonProperty" },
        dataProvider: { type: "string" },
      },
    };
    const fields = fieldsOfSchema(definition, { ParkingCategory: { enum: ["street", "garage"] } });
    expect(fields.map((f) => [f.attr, f.kind, f.required])).toEqual([
      ["name", "text", true],
      ["capacity", "integer", false],
      ["category", "enum", false],
      ["opened", "datetime", false],
      ["refOperator", "relationship", false],
      ["location", "point", false],
    ]);
    expect(fields[0].help).toBe("What it is called");
    expect(fields[2].options).toEqual([{ value: "street" }, { value: "garage" }]);
    expect(fieldsOfSchema(undefined, {})).toEqual([]);
  });
});

describe("a public form's trap and minted id", () => {
  it("names a trap no field of the form uses", () => {
    expect(trapName([])).toBe("website");
    expect(trapName([{ attr: "website", label: "w", kind: "text", required: false } satisfies FormField])).toBe("homepageUrl");
  });

  it("reads the created id from Location, or none", () => {
    expect(createdId("/ngsi-ld/v1/entities/urn%3Angsi-ld%3AReport%3Aa1")).toBe("urn:ngsi-ld:Report:a1");
    expect(createdId("https://x.example/api/endpoint/s/ngsi-ld/v1/entities/urn:ngsi-ld:Report:b2?x=1")).toBe("urn:ngsi-ld:Report:b2");
    expect(createdId(null)).toBeUndefined();
    expect(createdId("/elsewhere")).toBeUndefined();
  });
});
