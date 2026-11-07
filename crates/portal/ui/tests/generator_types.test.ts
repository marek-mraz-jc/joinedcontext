/**
 * What the app generator derives before a run starts: the types an app may read and the name it
 * gets when the person types none.
 */
import { describe, expect, it } from "vitest";
import { concreteTypes, slugOf } from "../src/pages/apps/AppGenerator";

describe("the types an app can read", () => {
  it("leaves out the base class and a definition marked abstract", () => {
    const schema = {
      $defs: {
        Entity: { properties: { id: {}, type: {} } },
        Place: { abstract: true, properties: { location: {} } },
        BikeHireDockingStation: { properties: { name: {}, availableBikeNumber: {} } },
        Alert: { properties: { category: {} } },
      },
    };
    expect(concreteTypes(schema).map((type) => type.name).sort()).toEqual([
      "Alert",
      "BikeHireDockingStation",
    ]);
  });

  it("reads a draft-07 document's `definitions` as the gateway serves an endpoint's view (T-3270)", () => {
    const schema = {
      definitions: {
        Place: { abstract: true, properties: { location: {} } },
        Article: { properties: { headline: {}, datePublished: {} } },
      },
    };
    expect(concreteTypes(schema)).toEqual([{ name: "Article", attributes: ["datePublished", "headline"] }]);
  });

  it("is empty for a document that is not a schema", () => {
    expect(concreteTypes(undefined)).toEqual([]);
  });
});

describe("the default app name", () => {
  it("is made of what the app is, not of the request's verbs", () => {
    expect(slugOf("Create a map of the bike stations with free slots")).toBe("map-bike-stations");
    expect(slugOf("generate a new visualization")).toBe("visualization");
    expect(slugOf("Generate a map of alerts")).toBe("map-alerts");
  });

  it("falls back to the endpoint's app when only filler is left", () => {
    expect(slugOf("create a new app for me", "helsinki-bikes")).toBe("helsinki-bikes-app");
    expect(slugOf("", "")).toBe("app");
  });

  it("stays a valid name: lower case, ascii, dashes, at most 40 characters", () => {
    const slug = slugOf("Vytvor mapu staníc bicyklov Žilina a Košice okolie");
    expect(slug).toMatch(/^[a-z0-9]+(-[a-z0-9]+)*$/);
    expect(slug).toBe("mapu-stanic-bicyklov");
    expect(slugOf("x".repeat(60)).length).toBeLessThanOrEqual(40);
  });
});
