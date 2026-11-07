// covers: src/schemas/kinds.ts
/** F-UI3-008: the App form admits a CSP source exactly when jc-core does (app.rs CSP_SOURCE, AP-12). */
import { describe, expect, it } from "vitest";
import { CSP_SOURCE_PATTERN } from "../src/schemas/kinds";

describe("CSP_SOURCE_PATTERN", () => {
  const admits = (value: string) => new RegExp(CSP_SOURCE_PATTERN).test(value);
  it("refuses a value that could open a directive of its own", () => {
    for (const bad of ["https://a;script-src", "https://a.example 'unsafe-eval'", "https://a\"b", "https://*.example", "http://a.example", "https://"]) {
      expect(admits(bad), bad).toBe(false);
    }
  });
  it("admits self, none and an https origin with a port and a path", () => {
    for (const good of ["self", "none", "https://tiles.example", "https://tiles.example:8443/v1/"]) {
      expect(admits(good), good).toBe(true);
    }
  });
});
