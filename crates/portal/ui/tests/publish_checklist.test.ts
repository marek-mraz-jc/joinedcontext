// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/apps/publishChecklist.ts.
/** T-3267, AP-140: what an App lacks before it is published, and what holds it. */
import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { CONTACT_ANNOTATION, LICENCE_ANNOTATION, mayPublish, publishChecklist } from "../src/pages/apps/publishChecklist";

const app = (metadata: object, spec: object): Manifest =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "App", metadata: { name: "events", ...metadata }, spec }) as Manifest;
const endpoint = (space: string, audience: string): Manifest =>
  ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: `${space}-${audience}` }, spec: { contextSpaceRef: space, audience } }) as Manifest;
const states = (items: ReturnType<typeof publishChecklist>) => Object.fromEntries(items.map((i) => [i.reason ?? i.key, i.state]));

describe("the publish checklist (T-3267)", () => {
  it("warns of every missing item and still lets a project app be published", () => {
    const items = publishChecklist(app({}, { visibility: "project" }), { endpoints: [] });
    expect(states(items)).toEqual({
      title: "warning",
      description: "warning",
      licence: "warning",
      visibility: "ok",
      checks: "warning",
      contact: "warning",
    });
    expect(mayPublish(items)).toBe(true);
  });

  it("reads the licence and contact from the App first, the installation's after", () => {
    const own = publishChecklist(
      app({ title: "Events", description: { sk: "Podujatia" }, annotations: { [LICENCE_ANNOTATION]: "CC0-1.0", [CONTACT_ANNOTATION]: "data@city.example" } }, {}),
      { licenceDefault: "CC-BY-4.0", contactEmail: "hello@city.example", check: { name: "events", state: "green", at: "2026-10-07T10:00:00Z" }, endpoints: [] },
    );
    expect(own.every((i) => i.state === "ok")).toBe(true);
    expect(own.find((i) => i.key === "licence")?.detail).toBe("CC0-1.0");
    const fallback = publishChecklist(app({}, {}), { licenceDefault: "CC-BY-4.0", contactEmail: "hello@city.example", endpoints: [] });
    expect(fallback.find((i) => i.key === "licence")?.detail).toBe("CC-BY-4.0");
    expect(fallback.find((i) => i.key === "contact")?.state).toBe("ok");
  });

  it("holds a private app, and a public one that writes, shows unpublished data or a person's own", () => {
    expect(mayPublish(publishChecklist(app({}, { visibility: "private" }), { endpoints: [] }))).toBe(false);
    const items = publishChecklist(
      app({}, {
        visibility: "public",
        dataNeeds: [
          { contextSpaceRef: "events", types: ["Event"], operations: ["queryEntity"] },
          { contextSpaceRef: { name: "people" }, types: ["Person"], attrs: ["name", "email", "telephone"], operations: ["createEntity"] },
        ],
      }),
      { endpoints: [endpoint("events", "public"), endpoint("people", "organization")] },
    );
    const privacy = items.filter((i) => i.key === "privacy").map((i) => `${i.reason}:${i.detail}`);
    expect(privacy).toEqual(["writes:people", "unpublished:people", "personal:email", "personal:telephone"]);
    expect(mayPublish(items)).toBe(false);
  });

  it("holds a public app for every operation and group jc-core counts as a write (F-UI2-015)", () => {
    for (const operation of ["purgeEntity", "redirectionOps", "updateOps", "deleteEntity", "mergeBatch", "upsertTemporal", "createSubscription"]) {
      const items = publishChecklist(
        app({}, { visibility: "public", dataNeeds: [{ contextSpaceRef: "events", operations: [operation] }] }),
        { endpoints: [endpoint("events", "public")] },
      );
      expect(items.filter((i) => i.key === "privacy").map((i) => i.reason), operation).toEqual(["writes"]);
    }
    const reads = publishChecklist(
      app({}, { visibility: "public", dataNeeds: [{ contextSpaceRef: "events", operations: ["retrieveOps", "queryEntity", "retrieveTemporal"] }] }),
      { endpoints: [endpoint("events", "public")] },
    );
    expect(reads.find((i) => i.key === "privacy")).toEqual({ key: "privacy", state: "ok" });
  });

  it("finds no privacy problem in a public app reading a published space, and none to look for in a project app", () => {
    const published = publishChecklist(app({}, { visibility: "public", dataNeeds: [{ contextSpaceRef: "events", attrs: ["name", "address"] }] }), {
      endpoints: [endpoint("events", "public")],
    });
    expect(published.find((i) => i.key === "privacy")).toEqual({ key: "privacy", state: "ok" });
    expect(publishChecklist(app({}, { visibility: "organization" }), { endpoints: [] }).some((i) => i.key === "privacy")).toBe(false);
  });
});
