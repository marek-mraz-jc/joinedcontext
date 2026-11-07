/** T-2890: a space's size as the broker measured it, in binary units and matched to the model's types. */
import { describe, expect, it } from "vitest";
import { bytesOfType, entityBytes, exactBytes, sizeText } from "../src/api/spaceUsage";

describe("space size", () => {
  it("writes_binary_units_with_the_locales_separators", () => {
    expect(sizeText(0, "en")).toBe("0 B");
    expect(sizeText(1023, "en")).toBe("1,023 B");
    expect(sizeText(1024, "en")).toBe("1 KiB");
    expect(sizeText(1536, "sk")).toBe("1,5 KiB");
    expect(sizeText(340 * 1024, "en")).toBe("340 KiB");
    expect(sizeText(3 * 1024 ** 4 * 2048, "en")).toBe("6,144 TiB");
    expect(exactBytes(1_572_864, "en")).toBe("1,572,864 B");
  });

  it("matches_a_short_type_to_its_stored_iri_and_never_guesses_between_two", () => {
    const byType = {
      "https://smartdatamodels.org/dataModel.Environment/AirQualityObserved": 700,
      "https://x/a#Room": 200,
      "https://y/Room": 100,
      Bridge: 50,
    };
    expect(bytesOfType(byType, "AirQualityObserved")).toBe(700);
    expect(bytesOfType(byType, "Bridge")).toBe(50);
    expect(bytesOfType(byType, "https://y/Room")).toBe(100);
    expect(bytesOfType(byType, "Room")).toBeUndefined();
    expect(bytesOfType(byType, "Missing")).toBeUndefined();
    expect(bytesOfType(undefined, "Bridge")).toBeUndefined();
  });

  it("weighs_a_sample_entity_as_its_utf8_json", () => {
    expect(entityBytes({ id: "urn:ngsi-ld:Bridge:1" })).toBe(29);
    expect(entityBytes({ name: "Šafárik" })).toBe(20);
    expect(entityBytes(undefined)).toBe(0);
  });
});
