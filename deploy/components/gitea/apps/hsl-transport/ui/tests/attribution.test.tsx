/**
 * T-3045: axe on dev found `link-in-text-block` (serious) on this App's map. MapLibre writes its
 * attribution itself, a link told apart from the line around it by colour alone. jsdom draws no
 * map, so what is held is the rule that dresses it in the App's own stylesheet.
 */
import { describe, expect, it } from "vitest";
import css from "../src/index.css?raw";

describe("the map's own attribution", () => {
  it("underlines its link, because colour is not the only way to tell it from the text", () => {
    expect(css).toMatch(/\.maplibregl-ctrl-attrib a\s*\{[^}]*text-decoration:\s*underline/);
  });
});
