// T-7589: the builder model names a refusal of the whole panel and of its colour thresholds.
import { beforeAll, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { emptyBoard, fieldWords } from "../src/pages/grafana/builderModel";

describe("the builder's refusal words", () => {
  beforeAll(async () => {
    await i18n.changeLanguage("en");
  });
  const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);

  it("names a refusal of the panel as a whole", () => {
    expect(fieldWords("panels[0]", emptyBoard(), t)).toBe(`Panel “${i18n.t("builder.untitledPanel")}”, Panel`);
  });

  it("names a refusal inside the colour thresholds as the colours by value", () => {
    expect(fieldWords("panels[0].thresholds[1].color", emptyBoard(), t)).toBe(`Panel “${i18n.t("builder.untitledPanel")}”, Colours by value`);
  });
});
