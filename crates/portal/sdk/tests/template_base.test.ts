import { describe, expect, it } from "vitest";
import template from "../template/vite.config";

// AP-14 (T-2724): a run cannot write `vite.config.ts`, so every generated app builds with the
// template's. An app is the whole of its own host (AP-133, T-2838), so its assets are asked of
// the root, from every path of the app.
describe("the template's build", () => {
  it("writes asset URLs from the root of the app's host", () => {
    expect(template).toMatchObject({ base: "/" });
  });
});
