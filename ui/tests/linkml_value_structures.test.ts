/**
 * A Smart Data Model import's value structures (L1..L3, the shape of a JsonProperty's value) are
 * no entity type, so the editor asks no class_uri of them (owner 2026-11-01, T-6353).
 */
import { describe, expect, it } from "vitest";
import { diagnose } from "../src/pages/models/linkml";

const SOURCE = `id: https://smartdatamodels.org/dataModel.Weather
name: weather
prefixes:
  sdm: https://smartdatamodels.org/
classes:
  WeatherObserved:
    class_uri: sdm:WeatherObserved
    slots: [precipitation]
  PrecipitationL1:
    slots: [amount]
  Station:
    slots: [amount]
slots:
  precipitation:
    range: PrecipitationL1
    inlined: true
  amount:
    range: float
`;

describe("class_uri on an import's value structures", () => {
  it("asks none of a value structure and still asks it of an entity type", () => {
    const missing = diagnose(SOURCE)
      .filter((d) => d.message.includes("has no class_uri"))
      .map((d) => d.path);
    expect(missing).toEqual(["classes.Station"]);
  });
});
