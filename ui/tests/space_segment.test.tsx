import { describe, expect, it } from "vitest";
import { spaceSegment } from "../src/routes/SpacesPage";

// PF-84: a space's entity ids carry `{project}-{name}` unless the space pins another segment.
describe("space segment", () => {
  it("renders project and local name when there is no pin", () => {
    expect(spaceSegment("helsinki", "bikes")).toBe("helsinki-bikes");
  });

  it("keeps the pin of a space that predates the rule", () => {
    expect(spaceSegment("helsinki", "helsinki-hub", "helsinki-hub")).toBe("helsinki-hub");
  });
});
