import { expect, it } from "vitest";
import { go } from "./go";

it("opens the address in this tab", () => {
  go("#downloaded");
  expect(window.location.hash).toBe("#downloaded");
});
