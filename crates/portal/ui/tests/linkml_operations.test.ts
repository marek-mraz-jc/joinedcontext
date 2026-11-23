/** T-0575: the operations a person, a test and an assistant edit a model with (DM-13, DM-16, DM-31). */
import { describe, expect, it } from "vitest";
import { diagnose, edit, graphData, mergeModels, parseModel } from "../src/pages/models/linkml";
import { applyOperations } from "../src/pages/models/operations";
import type { Operation } from "../src/pages/models/operations";

const SOURCE = `# the model the city publishes
id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots:
      - pm10
slots:
  pm10:
    range: float
    slot_uri: bb:pm10
enums: {}
`;

function slotNamed(source: string, name: string) {
  return parseModel(source).slots.find((slot) => slot.name === name);
}

describe("LinkML operations", () => {
  it("adds a class and a slot with two operations, and keeps the comment", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "addClass", name: "WeatherObserved", class_uri: "bb:WeatherObserved" },
      { op: "addSlot", name: "temperature", class: "WeatherObserved", range: "float", kind: "Property" },
    ]);
    expect(refused).toEqual([]);
    const model = parseModel(source);
    expect(model.classes.map((klass) => klass.name)).toEqual(["AirQualityObserved", "WeatherObserved"]);
    expect(model.classes[1].slots).toEqual(["temperature"]);
    expect(model.classes[1].class_uri).toBe("bb:WeatherObserved");
    expect(slotNamed(source, "temperature")?.range).toBe("float");
    expect(source).toContain("# the model the city publishes");
  });

  it("sets every slot field the form offers", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "setSlot", name: "pm10", field: "required", value: true },
      { op: "setSlot", name: "pm10", field: "multivalued", value: true },
      { op: "setSlot", name: "pm10", field: "description", value: "particulate matter" },
      { op: "setSlot", name: "pm10", field: "unit", value: "GQ" },
      { op: "setSlot", name: "pm10", field: "kind", value: "Property" },
      { op: "setSlot", name: "pm10", field: "minimum_value", value: 0 },
      { op: "setTitle", target: "slot", name: "pm10", locale: "sk", value: "PM10" },
      { op: "setTitle", target: "class", name: "AirQualityObserved", locale: "en", value: "Air quality" },
    ]);
    expect(refused).toEqual([]);
    const slot = slotNamed(source, "pm10");
    expect(slot).toMatchObject({
      required: true,
      multivalued: true,
      description: "particulate matter",
      // The wire value and the anchor together (DM-59): the UN/CEFACT code NGSI-LD carries and
      // the QUDT unit a federated reader dereferences, with the dimension beside it.
      unit: {
        ucum_code: "ug.m-3",
        exact_mappings: ["ucefact:GQ", "qudt-unit:MicroGM-PER-M3"],
        has_quantity_kind: "qudt-quantkind:Density",
      },
      kind: "Property",
      minimum_value: 0,
      title: { sk: "PM10" },
    });
    expect(parseModel(source).classes[0].title).toEqual({ en: "Air quality" });
    // The default kind is not written: a Property annotation would be noise in every model.
    expect(source).not.toContain("ngsi_ld_kind");

    const cleared = applyOperations(source, [
      { op: "setSlot", name: "pm10", field: "required", value: false },
      { op: "setSlot", name: "pm10", field: "unit", value: "" },
      { op: "setSlot", name: "pm10", field: "kind", value: "GeoProperty" },
    ]).source;
    expect(slotNamed(cleared, "pm10")).toMatchObject({ required: false, unit: undefined, kind: "GeoProperty" });
  });

  it("takes any Rec 20 code, writes only what the code list carries, and refuses a code by the wrong case", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "setSlot", name: "pm10", field: "unit", value: "A97" },
      { op: "addSlot", name: "bikes", range: "integer" },
      { op: "setSlot", name: "bikes", field: "unit", value: "H87" },
    ]);
    expect(refused).toEqual([]);
    const wrongCase = applyOperations(SOURCE, [{ op: "setSlot", name: "pm10", field: "unit", value: "gq" }]);
    expect(wrongCase.source).toBe(SOURCE);
    expect(wrongCase.refused.map((refusal) => refusal.reason)).toEqual([
      "unit 'gq' of slot 'pm10' is not a UN/CEFACT Recommendation 20 code; codes are case-sensitive, such as GQ for µg/m³ or CEL for °C",
    ]);
    expect(slotNamed(source, "pm10")?.unit).toEqual({
      ucum_code: "hPa",
      exact_mappings: ["ucefact:A97", "qudt-unit:HectoPA"],
      has_quantity_kind: "qudt-quantkind:ForcePerArea",
    });
    // A piece has no UCUM code, QUDT unit or quantity kind in the list: none is invented.
    expect(slotNamed(source, "bikes")?.unit).toEqual({ exact_mappings: ["ucefact:H87"] });
  });

  it("removes, attaches and detaches, and enums get their values", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "addEnum", name: "QualityBand" },
      { op: "addEnumValue", enum: "QualityBand", value: "good", description: "below the limit" },
      { op: "addSlot", name: "band", range: "QualityBand" },
      { op: "attachSlot", class: "AirQualityObserved", slot: "band" },
      { op: "attachSlot", class: "AirQualityObserved", slot: "band" },
      { op: "detachSlot", class: "AirQualityObserved", slot: "pm10" },
      { op: "removeSlot", name: "pm10" },
    ]);
    expect(refused).toEqual([]);
    const model = parseModel(source);
    expect(model.classes[0].slots).toEqual(["band"]);
    expect(model.slots.map((slot) => slot.name)).toEqual(["band"]);
    expect(model.enums[0].permissible_values).toEqual([
      { name: "good", meaning: undefined, description: "below the limit" },
    ]);

    const gone = applyOperations(source, [{ op: "removeClass", name: "AirQualityObserved" }]).source;
    expect(parseModel(gone).classes).toEqual([]);
  });

  it("renames a slot in every class and a class in every reference, keeping places and comments", () => {
    const withReference = SOURCE.replace(
      "enums: {}\n",
      "  station:\n    range: AirQualityObserved\nenums: {}\n",
    ).replace("    class_uri: bb:AirQualityObserved\n", "    class_uri: bb:AirQualityObserved\n    is_a: Entity\n");
    const { source, refused } = applyOperations(withReference, [
      { op: "addClass", name: "Reading", is_a: "AirQualityObserved" },
      { op: "renameSlot", name: "pm10", to: "particles" },
      { op: "renameClass", name: "AirQualityObserved", to: "AirQuality" },
    ]);
    expect(refused).toEqual([]);
    const model = parseModel(source);
    expect(model.classes.map((klass) => klass.name)).toEqual(["AirQuality", "Reading"]);
    expect(model.classes[0].slots).toEqual(["particles"]);
    expect(model.slots.map((slot) => slot.name)).toEqual(["particles", "station"]);
    expect(slotNamed(source, "particles")?.range).toBe("float");
    expect(slotNamed(source, "station")?.range).toBe("AirQuality");
    expect(source).toContain("is_a: Entity");
    expect(source).toContain("  Reading:\n    is_a: AirQuality");
    expect(source).toContain("# the model the city publishes");

    expect(
      applyOperations(SOURCE, [
        { op: "renameSlot", name: "pm25", to: "x" },
        { op: "renameClass", name: "AirQualityObserved", to: "AirQualityObserved" },
      ]).refused.map((refusal) => refusal.reason),
    ).toEqual(["unknown slot 'pm25'", "class 'AirQualityObserved' already exists"]);
  });

  it("moves a class's slot_usage with a renamed slot and drops it with a removed one (F-R2UI3-008)", () => {
    const narrowed = SOURCE.replace("      - pm10\n", "      - pm10\n    slot_usage:\n      pm10:\n        required: true\n");
    const renamed = applyOperations(narrowed, [{ op: "renameSlot", name: "pm10", to: "particles" }]);
    expect(renamed.refused).toEqual([]);
    expect(parseModel(renamed.source).classes[0].slot_usage).toEqual({ particles: expect.objectContaining({ required: true }) });
    const removed = applyOperations(narrowed, [{ op: "removeSlot", name: "pm10" }]);
    expect(removed.refused).toEqual([]);
    expect(parseModel(removed.source).classes[0].slot_usage).toEqual({});
  });

  it("adds a slot with its kind and an entity class in one operation each", () => {
    // T-0599: inference answers `addSlot` with a kind and `addClass` with `is_a`, so the two
    // must land in one operation, not need a second `setSlot`.
    const applied = applyOperations(SOURCE, [
      { op: "addClass", name: "Reading", is_a: "Entity" },
      { op: "addSlot", name: "parent", class: "Reading", range: "uriorcurie", kind: "Relationship" },
    ]);

    expect(applied.refused).toEqual([]);
    const model = parseModel(applied.source);
    expect(model.slots.find((slot) => slot.name === "parent")?.kind).toBe("Relationship");
    expect(applied.source).toContain("is_a: Entity");
  });

  it("refuses by name, and then nothing of the list lands", () => {
    const operations: Operation[] = [
      { op: "addClass", name: "WeatherObserved" },
      { op: "attachSlot", class: "Nope", slot: "pm10" },
      { op: "setSlot", name: "pm25", field: "range", value: "float" },
      { op: "setSlot", name: "pm10", field: "slot_uri", value: "sdm:pm10" },
      { op: "setSlot", name: "pm10", field: "range", value: "Nowhere" },
      { op: "setSlot", name: "pm10", field: "unit", value: "XX" },
      { op: "addClass", name: "not a name" },
      { op: "addSlot", name: "pm10" },
      { op: "addEnumValue", enum: "Missing", value: "x" },
    ];
    const { source, refused } = applyOperations(SOURCE, operations);
    expect(source).toBe(SOURCE);
    expect(refused.map((refusal) => refusal.index)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(refused.map((refusal) => refusal.reason)).toEqual([
      "unknown class 'Nope'",
      "unknown slot 'pm25'",
      "slot 'pm10' would mint 'sdm:pm10' under https://smartdatamodels.org/, which belongs to someone else",
      "range 'Nowhere' of slot 'pm10' is neither a type, an enum nor a class of this model",
      "unit 'XX' of slot 'pm10' is not a UN/CEFACT Recommendation 20 code; codes are case-sensitive, such as GQ for µg/m³ or CEL for °C",
      "'not a name' is not a valid class name: letters, digits and _ only, starting with a letter, " +
        "because the name becomes the entity type in every URN. Put the readable name in its title.",
      "slot 'pm10' already exists",
      "unknown enum 'Missing'",
    ]);
  });

  it("refuses each wrong value type, duplicate and unknown choice with its own reason", () => {
    const base = applyOperations(SOURCE, [
      { op: "addEnum", name: "Level" },
      { op: "addEnumValue", enum: "Level", value: "low" },
      { op: "addClass", name: "Station" },
      { op: "addSlot", name: "pm25", class: "AirQualityObserved", range: "float" },
      { op: "addSlot", name: "station", class: "AirQualityObserved", range: "Station", kind: "Relationship" },
      { op: "addSlot", name: "twin", class: "Station", range: "Station", kind: "Relationship" },
    ]);
    expect(base.refused).toEqual([]);
    const withTwoOwners = applyOperations(base.source, [{ op: "attachSlot", class: "Station", slot: "station" }]);
    expect(withTwoOwners.refused).toEqual([]);
    const cases: Array<[Operation, string]> = [
      [{ op: "setSlot", name: "pm10", field: "required", value: "yes" }, "'required' of slot 'pm10' takes true or false"],
      [{ op: "setSlot", name: "pm10", field: "description", value: 4 }, "'description' of slot 'pm10' takes text"],
      [{ op: "setSlot", name: "pm10", field: "minimum_value", value: "0" }, "'minimum_value' of slot 'pm10' takes a number"],
      [{ op: "setSlot", name: "pm10", field: "unit", value: 3 }, "'unit' of slot 'pm10' takes text"],
      [{ op: "setSlot", name: "pm10", field: "kind", value: "Wrong" }, "kind 'Wrong' of slot 'pm10' is not one of "],
      [{ op: "setSlot", name: "pm10", field: "bogus" as never, value: "x" }, "'bogus' is not a slot field the editor sets"],
      [{ op: "addClass", name: "Station" }, "class 'Station' already exists"],
      [{ op: "renameSlot", name: "pm10", to: "pm25" }, "slot 'pm25' already exists"],
      [{ op: "detachSlot", class: "Station", slot: "pm10" }, "class 'Station' does not use slot 'pm10'"],
      [{ op: "addEnum", name: "Level" }, "enum 'Level' already exists"],
      [{ op: "addEnumValue", enum: "Level", value: "low" }, "enum 'Level' already has the value 'low'"],
      [{ op: "setCardinality", name: "station", cardinality: "many" as never }, "'many' is not a cardinality: "],
      [{ op: "setOnDelete", name: "station", onDelete: "explode" as never }, "'explode' is not a delete rule: "],
      [{ op: "addInverse", name: "station", inverse: "seen" }, "slot 'station' is used by 2 classes; a relationship starts on one"],
      [{ op: "nothing" } as unknown as Operation, "unknown operation 'nothing'"],
    ];
    for (const [operation, reason] of cases) {
      const { source, refused } = applyOperations(withTwoOwners.source, [operation]);
      expect(source).toBe(withTwoOwners.source);
      expect(refused).toHaveLength(1);
      expect(refused[0].reason.startsWith(reason)).toBe(true);
    }
    // A range may name an enum of the model.
    const ranged = applyOperations(base.source, [{ op: "setSlot", name: "pm10", field: "range", value: "Level" }]);
    expect(ranged.refused).toEqual([]);
    expect(slotNamed(ranged.source, "pm10")?.range).toBe("Level");
    // A slot used by no class cannot start a relationship.
    const loose = applyOperations(base.source, [
      { op: "addSlot", name: "orphan", range: "Station", kind: "Relationship" },
      { op: "addInverse", name: "orphan", inverse: "back" },
    ]);
    expect(loose.refused.map((refusal) => refusal.reason)).toEqual([
      "slot 'orphan' is used by 0 classes; a relationship starts on one",
    ]);
  });

  it("lets a slot cite an upstream term under a reserved namespace, and refuses a broken source", () => {
    const cited = SOURCE.replace(
      "  pm10:\n",
      "  no2:\n    annotations:\n      upstream_source: sdm\n  pm10:\n",
    );
    const { refused } = applyOperations(cited, [
      { op: "setSlot", name: "no2", field: "slot_uri", value: "sdm:no2" },
    ]);
    expect(refused).toEqual([]);

    const broken = applyOperations("classes: [\n", [{ op: "addClass", name: "X" }]);
    expect(broken.source).toBe("classes: [\n");
    expect(broken.refused).toEqual([{ index: 0, reason: "the source does not parse as YAML" }]);
  });
});

/**
 * T-1091: a class name becomes the entity type in every URN the space mints, and the gateway
 * accepts `^[A-Za-z][A-Za-z0-9_-]*$`, so an accented name would mint ids its own endpoint
 * refuses. The refusal says that, and says where the accented name belongs.
 */
describe("a name that could not survive a URN", () => {
  it("refuses an accented class name and says where the readable name goes", () => {
    const { refused } = applyOperations(SOURCE, [
      { op: "addClass", name: "Ovzduší" } as Operation,
    ]);
    expect(refused).toHaveLength(1);
    const reason = refused[0].reason;
    expect(reason).toContain("Ovzduší");
    expect(reason).toContain("entity type");
    expect(reason).toContain("title");
  });

  it("refuses the other shapes a name cannot take, and accepts the ones it can", () => {
    for (const name of ["东京", "Ärzte", "2Wheels", "air quality", "", "air-quality"]) {
      const { refused } = applyOperations(SOURCE, [{ op: "addClass", name } as Operation]);
      expect(refused, `${name} is not a usable class name`).toHaveLength(1);
    }
    for (const name of ["NoiseObserved", "_internal", "Bike2"]) {
      const { refused } = applyOperations(SOURCE, [{ op: "addClass", name } as Operation]);
      expect(refused, `${name} is a usable class name`).toEqual([]);
    }
  });
});

/**
 * T-1085, DM-13: the metamodel the editor reads and writes — a class's `is_a` and `mixins`, a
 * slot's `subsets`, the model's `imports`. They were parsed by nobody, so the visual editor
 * could not show a hierarchy the YAML plainly declared.
 */
describe("the hierarchy a model declares", () => {
  const WITH_HIERARCHY = `${SOURCE}
imports:
  - linkml:types
`;

  it("reads is_a, mixins, subsets and imports off the source", () => {
    const withParent = applyOperations(WITH_HIERARCHY, [
      { op: "addClass", name: "Station" } as Operation,
      { op: "setClass", name: "AirQualityObserved", field: "is_a", value: "Station" } as Operation,
      { op: "setClassMixins", name: "AirQualityObserved", mixins: ["Station"] } as Operation,
      { op: "setSlotSubsets", name: "pm10", subsets: ["public", "steward"] } as Operation,
    ]);
    expect(withParent.refused).toEqual([]);

    const model = parseModel(withParent.source);
    const observed = model.classes.find((klass) => klass.name === "AirQualityObserved");
    expect(observed?.is_a).toBe("Station");
    expect(observed?.mixins).toEqual(["Station"]);
    expect(model.slots.find((slot) => slot.name === "pm10")?.subsets).toEqual([
      "public",
      "steward",
    ]);
    expect(model.imports).toEqual(["linkml:types"]);
  });

  it("refuses a hierarchy that names nothing, or names itself", () => {
    for (const operation of [
      { op: "setClass", name: "AirQualityObserved", field: "is_a", value: "Nowhere" },
      { op: "setClass", name: "AirQualityObserved", field: "is_a", value: "AirQualityObserved" },
      { op: "setClassMixins", name: "AirQualityObserved", mixins: ["AirQualityObserved"] },
      { op: "setClassMixins", name: "AirQualityObserved", mixins: ["Nowhere"] },
    ] as Operation[]) {
      const { refused } = applyOperations(SOURCE, [operation]);
      expect(refused, JSON.stringify(operation)).toHaveLength(1);
    }
  });

  it("empties a list by removing the key, never by leaving it behind", () => {
    const set = applyOperations(SOURCE, [
      { op: "addClass", name: "Station" } as Operation,
      { op: "setClassMixins", name: "AirQualityObserved", mixins: ["Station"] } as Operation,
    ]);
    const cleared = applyOperations(set.source, [
      { op: "setClassMixins", name: "AirQualityObserved", mixins: [] } as Operation,
    ]);
    expect(cleared.refused).toEqual([]);
    expect(cleared.source).not.toContain("mixins");
    expect(parseModel(cleared.source).classes.find((k) => k.name === "AirQualityObserved")?.mixins).
      toBeUndefined();
  });
});

/**
 * T-1102, DM-07: importing a second catalogue model joins it to the one being edited, so a
 * Vehicle and an AirQualityObserved can sit in one model and a slot can relate them. Importing
 * used to replace the source, which is why two imported models could never be connected.
 */
describe("what the parser and the diagnostics say about odd slots", () => {
  const HEAD = "id: https://x.test/m\nname: m\nprefixes:\n  x: https://x.test/\n  sdm: https://smartdatamodels.org/\n  schema: https://schema.org/\n";

  it("reads what a class narrows in slot_usage, and a label given as plain text", () => {
    const model = parseModel(
      `${HEAD}classes:\n  A:\n    class_uri: x:A\n    slots: [s]\n    slot_usage:\n      s:\n        range: integer\n        multivalued: true\n        description: narrowed\n        annotations:\n          label: Plain label\n      t:\n        annotations:\n          label: "  "\nslots:\n  s: { slot_uri: x:s }\n`,
    );
    expect(model.classes[0].slot_usage?.s).toMatchObject({
      range: "integer",
      multivalued: true,
      description: "narrowed",
      label: { "": "Plain label" },
    });
    expect(model.classes[0].slot_usage?.t.label).toBeUndefined();
  });

  it("warns about a slot without a unit code and refuses a reserved IRI and an unknown kind", () => {
    const source = `${HEAD}slots:\n  temperature:\n    range: float\n    slot_uri: x:temperature\n    unit: { symbol: "deg" }\n  borrowed:\n    range: string\n    slot_uri: sdm:name\n  odd:\n    range: string\n    slot_uri: x:odd\n    annotations:\n      ngsi_ld_kind: Wrong\n`;
    const messages = diagnose(source).map((diagnostic) => `${diagnostic.severity}: ${diagnostic.message}`);
    expect(messages).toContain(
      "warning: slot 'temperature' declares a unit without a UN/CEFACT common code, so exports and dashboards cannot label it",
    );
    expect(messages.some((message) => message.startsWith("error: slot 'borrowed' mints 'sdm:name' under https://smartdatamodels.org/"))).toBe(true);
    expect(messages.some((message) => message.includes("slot 'odd' declares ngsi_ld_kind 'Wrong'"))).toBe(true);
  });

  it("leaves a source that does not parse exactly as it was", () => {
    let called = false;
    expect(edit("classes: [\n", () => { called = true; })).toBe("classes: [\n");
    expect(called).toBe(false);
  });

  it("draws a listed slot nothing declares as a field of the model's default type, and refuses a slot that is its own inverse", () => {
    const loose = parseModel(`${HEAD}default_range: string\nclasses:\n  A:\n    class_uri: x:A\n    slots: [ghost]\n`);
    const rows = graphData(loose).nodes[0].rows ?? [];
    expect(rows.map((row) => `${row.name}:${row.type}`)).toEqual(["id:URN", "ghost:string"]);

    const own = `${HEAD}classes:\n  Person:\n    class_uri: x:Person\n    slots: [friend]\nslots:\n  friend:\n    range: Person\n    slot_uri: x:friend\n    inverse: friend\n    annotations:\n      ngsi_ld_kind: Relationship\n`;
    expect(diagnose(own).some((diagnostic) => diagnostic.rule === "inverse-not-reciprocal" && diagnostic.message.includes("names itself as its inverse"))).toBe(true);
  });
});

describe("a second imported model joined to the first", () => {
  const VEHICLE = `id: https://smartdatamodels.org/Vehicle
name: Vehicle
prefixes:
  sdm: https://smartdatamodels.org/
classes:
  Vehicle:
    class_uri: sdm:Vehicle
    slots: [vehicleType]
slots:
  vehicleType:
    range: string
enums:
  VehicleCategory:
    permissible_values:
      municipalServices: {}
`;

  it("adds what the incoming model has and keeps what the current one already had", () => {
    const { source, conflicts } = mergeModels(SOURCE, VEHICLE);
    const model = parseModel(source);

    // Both classes are there, so a slot can now relate one to the other.
    expect(model.classes.map((klass) => klass.name).sort()).toContain("Vehicle");
    expect(model.classes.map((klass) => klass.name)).toContain("AirQualityObserved");
    expect(model.slots.map((slot) => slot.name)).toContain("vehicleType");
    expect(model.enums.map((one) => one.name)).toContain("VehicleCategory");
    expect(model.prefixes.sdm).toBe("https://smartdatamodels.org/");
    expect(conflicts).toEqual([]);

    // And the connection itself is an operation that already exists.
    const related = applyOperations(source, [
      { op: "addSlot", name: "observedBy", range: "Vehicle", kind: "Relationship" } as Operation,
      { op: "attachSlot", class: "AirQualityObserved", slot: "observedBy" } as Operation,
    ]);
    expect(related.refused).toEqual([]);
    const observedBy = parseModel(related.source).slots.find((s) => s.name === "observedBy");
    expect(observedBy?.kind).toBe("Relationship");
    expect(observedBy?.range).toBe("Vehicle");
  });

  it("never overwrites what the person has edited, and says what it kept", () => {
    // The same model twice is complete agreement: nothing to add and nothing to report.
    expect(mergeModels(SOURCE, SOURCE)).toEqual({ source: SOURCE, conflicts: [] });

    // A slot of the same name defined differently is what the person must be told about: the
    // version in hand may have been edited, and the import must not undo that.
    const differing = VEHICLE.replace(
      "slots:\n  vehicleType:\n    range: string",
      "slots:\n  pm10:\n    range: integer",
    ).replace("slots: [vehicleType]", "slots: [pm10]");
    const { source, conflicts } = mergeModels(SOURCE, differing);
    expect(conflicts).toContainEqual({ section: "slots", name: "pm10" });
    // And the slot kept is the one that was already there.
    expect(parseModel(source).slots.find((slot) => slot.name === "pm10")?.range).toBe(
      parseModel(SOURCE).slots.find((slot) => slot.name === "pm10")?.range,
    );
  });

  it("leaves the model alone when the incoming source does not parse", () => {
    expect(mergeModels(SOURCE, "classes: [this is: not: a map").source).toBe(SOURCE);
  });
});

/**
 * DM-58: what a borrowed term means is the upstream's to say.
 *
 * A slot that cites an `upstream_source` has bound somebody else's IRI, and the citation is the
 * claim that this slot *is* that term. Keeping the IRI while changing what the slot holds makes
 * two incompatible things answer to one name, and the disagreement surfaces at the federation
 * partner reading our `@context`, not here. Checked at the edit because that is the only place
 * the citation and the change are both in hand.
 */
describe("a term this model did not define", () => {
  const BORROWED = `id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQualityObserved:
    slots:
      - pm10
slots:
  pm10:
    range: float
    slot_uri: sdm:pm10
    annotations:
      upstream_source: https://github.com/smart-data-models/dataModel.Environment@9f1c2b7
enums: {}
`;

  it("refuses to redefine what the borrowed term holds", () => {
    for (const [field, value] of [
      ["range", "string"],
      ["unit", "CEL"],
      ["required", true],
      ["multivalued", true],
    ] as const) {
      const { refused, source } = applyOperations(BORROWED, [
        { op: "setSlot", name: "pm10", field, value } as Operation,
      ]);
      expect(refused.map((one) => one.reason).join(" "), field).toContain("DM-58");
      expect(refused[0]?.reason, field).toContain("dataModel.Environment");
      // Refused means unchanged: the document that comes back is the one that went in.
      expect(source, field).toBe(BORROWED);
    }
  });

  it("allows what is not the upstream's to define, and everything on a term of our own", () => {
    const described = applyOperations(BORROWED, [
      { op: "setSlot", name: "pm10", field: "description", value: "particulate matter" },
    ]);
    expect(described.refused).toEqual([]);
    expect(slotNamed(described.source, "pm10")?.description).toBe("particulate matter");

    const ours = applyOperations(BORROWED, [
      { op: "addSlot", name: "stationHousing", class: "AirQualityObserved" },
      { op: "setSlot", name: "stationHousing", field: "range", value: "string" },
      { op: "setSlot", name: "stationHousing", field: "required", value: true },
    ]);
    expect(ours.refused).toEqual([]);
    expect(slotNamed(ours.source, "stationHousing")?.required).toBe(true);
  });
});
