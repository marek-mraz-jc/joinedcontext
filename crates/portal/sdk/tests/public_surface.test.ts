/**
 * What `@joinedcontext/sdk` promises an application (T-1063, T-1064, SDK-01, AP-56).
 *
 * The barrel is the contract: a generated application imports from it and from nothing else, and
 * an app in somebody's repository keeps working only while every name it holds keeps resolving.
 * Two of those names come from files a de-slop pass has already proposed deleting as "dead code"
 * — `compare` from `views/Table` and the five download helpers from `artifact` — so this walks
 * the whole surface rather than trusting that a grep found every reader.
 */
import { describe, expect, it } from "vitest";
import * as sdk from "../src/sdk/index";

/** Every value (not type) the barrel exports, by the module it stands on. */
const PROMISED: Record<string, string[]> = {
  "./client": ["jc", "ProblemError"],
  "./access": ["can"],
  "./tokens": [
    "applyTokens",
    "currentTokens",
    "DEFAULT_TOKENS",
    "echartsTheme",
    "mapColors",
    "rechartsPalette",
    "resolveTokens",
  ],
  "./hooks": [
    "JcProvider",
    "useAccess",
    "useClient",
    "useEntities",
    "useEntity",
    "useFunction",
    "useMe",
    "useSave",
    "useSchema",
  ],
  "./filters": ["filterRows", "useFilters"],
  "./helpers": [
    "aggregate",
    "columnKind",
    "displayName",
    "distinct",
    "extent",
    "format",
    "groupBy",
    "pointOf",
    "toFeatureCollection",
  ],
  "./map": ["mapWorkerReady", "NO_BASEMAP", "NO_LOCATIONS", "styleFor"],
  "./start": ["startApp"],
  "./report": ["reportError"],
  "../write": ["fieldOf"],
  // The two a "delete the dead file" pass would take with it.
  "../views/Table": ["compare"],
  "../artifact": ["download", "toCsv", "toGeoJson", "toPdf", "toPng"],
};

describe("the SDK's public surface", () => {
  it("resolves every name it promises", () => {
    const missing: string[] = [];
    for (const [module, names] of Object.entries(PROMISED)) {
      for (const name of names) {
        if ((sdk as Record<string, unknown>)[name] === undefined) {
          missing.push(`${name} (${module})`);
        }
      }
    }
    expect(missing).toEqual([]);
  });

  it("stands on the modules a deletion would have to answer for", () => {
    // Named rather than counted: a file removed under one of these names breaks an application
    // that is already deployed, and the failure belongs here rather than in somebody's repo.
    expect(typeof sdk.compare).toBe("function");
    expect(typeof sdk.toCsv).toBe("function");
    expect(typeof sdk.toGeoJson).toBe("function");
    expect(typeof sdk.download).toBe("function");
    expect(typeof sdk.startApp).toBe("function");
  });
});
