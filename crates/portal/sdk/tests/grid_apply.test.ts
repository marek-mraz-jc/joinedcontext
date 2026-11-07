/**
 * Applying edited cells through the Endpoint (T-1430; UI-67, EP-55, AG-78).
 *
 * The write path is the person's own session and the Endpoint's Policy, so what matters here is
 * what leaves the browser per entity and what comes back when one update is refused: a refusal
 * belongs to its entity, in the endpoint's own words, and the entities that landed stay landed.
 */
import { describe, expect, it } from "vitest";
import { applyChanges, attrsBody, MAX_ENTITIES, MAX_REASON } from "../src/grid/apply";
import { SourceError } from "../src/grid/source";

/** A source that records what it was asked to write and refuses whichever entity a test names. */
function recording(refuse: (id: string) => SourceError | undefined = () => undefined) {
  const sent: { id: string; attrs: Record<string, unknown> }[] = [];
  const reasons: (string | undefined)[] = [];
  const source = {
    patch: async (id: string, attrs: Record<string, unknown>, reason?: string) => {
      sent.push({ id, attrs });
      reasons.push(reason);
      const problem = refuse(id);
      if (problem) {
        throw problem;
      }
    },
  };
  return { sent, reasons, source };
}

const ONE = {
  id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
  changes: [{ attribute: "availableBikeNumber", before: 5, after: 4, unitCode: "C62" }],
};

describe("applying edited cells", () => {
  it("sends the person's reason with every update, trimmed (DM-113)", async () => {
    const { reasons, source } = recording();
    await applyChanges({ reason: "  Sensor stuck at 0  ", source, entities: [ONE, { id: "urn:ngsi-ld:Bike:hel:helsinki:002", changes: [{ attribute: "name", after: "x" }] }] });
    expect(reasons).toEqual(["Sensor stuck at 0", "Sensor stuck at 0"]);
  });

  it("sends nothing without a reason, or with one longer than the gateway takes", async () => {
    const { sent, source } = recording();
    await expect(applyChanges({ reason: "   ", source, entities: [ONE] })).rejects.toThrow(/names why/);
    await expect(applyChanges({ reason: "x".repeat(MAX_REASON + 1), source, entities: [ONE] })).rejects.toThrow(/names why/);
    expect(sent).toHaveLength(0);
  });

  it("sends one PATCH per entity, with only what changed and the unit it was measured in", async () => {
    const { sent, source } = recording();
    const result = await applyChanges({ reason: "Sensor stuck",
      source,
      entities: [ONE, { id: "urn:ngsi-ld:Bike:hel:helsinki:002", changes: [{ attribute: "name", after: "Kallio" }] }],
    });

    expect(result).toEqual({
      applied: ["urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001", "urn:ngsi-ld:Bike:hel:helsinki:002"],
      refused: [],
    });
    expect(sent).toHaveLength(2);
    expect(sent[0].id).toBe(ONE.id);
    // Only the edited attribute, and the unit kept: a number without its unit is another number.
    expect(sent[0].attrs).toEqual({
      availableBikeNumber: { type: "Property", value: 4, unitCode: "C62" },
    });
    expect(sent[1].attrs).toEqual({ name: { type: "Property", value: "Kallio" } });
  });

  it("writes a Relationship as one, and never gives it a unit", () => {
    expect(
      attrsBody([{ attribute: "refDevice", after: "urn:ngsi-ld:Device:hel:9", kind: "relationship", unitCode: "C62" }], "keep"),
    ).toEqual({ refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:hel:9" } });
  });

  it("dates a corrected value to now only when the person asked for it", () => {
    const now = () => "2026-09-19T09:00:00.000Z";
    expect(attrsBody(ONE.changes, "keep", now).availableBikeNumber).toEqual({
      type: "Property",
      value: 4,
      unitCode: "C62",
    });
    expect(attrsBody(ONE.changes, "now", now).availableBikeNumber).toEqual({
      type: "Property",
      value: 4,
      unitCode: "C62",
      observedAt: "2026-09-19T09:00:00.000Z",
    });
  });

  it("keeps a refusal with its own entity, in the endpoint's words", async () => {
    const { sent, source } = recording((id) =>
      id.endsWith("002")
        ? new SourceError(403, "availableBikeNumber is not yours to write")
        : undefined,
    );
    const result = await applyChanges({ reason: "Sensor stuck",
      source,
      entities: [ONE, { id: "urn:ngsi-ld:Bike:hel:helsinki:002", changes: [{ attribute: "name", after: "x" }] }],
    });

    // The first landed and stays landed: one refusal is not a refused batch.
    expect(result.applied).toEqual([ONE.id]);
    expect(result.refused).toEqual([
      { id: "urn:ngsi-ld:Bike:hel:helsinki:002", detail: "availableBikeNumber is not yours to write" },
    ]);
    expect(sent).toHaveLength(2);
  });

  it("says something useful when the refusal explains nothing", async () => {
    // The endpoint's own sentence is what the source carries; a refusal without one still names
    // the status, so a person is never told only that something went wrong.
    const { source } = recording(() => new SourceError(412, ""));
    const result = await applyChanges({ reason: "Sensor stuck", source, entities: [ONE], fallback: "the update was refused" });
    expect(result.refused).toEqual([{ id: ONE.id, detail: "the update was refused (412)" }]);

    const conflict = recording(() => new SourceError(409, "the entity changed since you read it"));
    const second = await applyChanges({ reason: "Sensor stuck", source: conflict.source, entities: [ONE] });
    expect(second.refused[0].detail).toBe("the entity changed since you read it");
    // And the cell stays the person's: nothing applied, so nothing is forgotten.
    expect(second.applied).toEqual([]);
  });

  it("refuses to apply to a surface that takes no writes", async () => {
    await expect(applyChanges({ reason: "Sensor stuck", source: {}, entities: [ONE] })).rejects.toThrow(/does not take writes/);
  });

  it("refuses a batch beyond the platform's own cap instead of sending half of it", async () => {
    const { sent, source } = recording();
    const many = Array.from({ length: MAX_ENTITIES + 1 }, (_, i) => ({
      id: `urn:ngsi-ld:Bike:hel:helsinki:${i}`,
      changes: [{ attribute: "name", after: `n${i}` }],
    }));
    await expect(applyChanges({ reason: "Sensor stuck", source, entities: many })).rejects.toThrow(/at most 50/);
    expect(sent).toHaveLength(0);
    // The cap itself: `MAX_ENTITIES` in `src/agents/entity_write.rs`.
    expect(MAX_ENTITIES).toBe(50);
  });

  it("sends nothing for an entity whose cells are all back to what they were", async () => {
    const { sent, source } = recording();
    const result = await applyChanges({ reason: "Sensor stuck", source, entities: [{ id: ONE.id, changes: [] }] });
    expect(sent).toHaveLength(0);
    expect(result).toEqual({ applied: [], refused: [] });
  });

  it("sends a value that looks like markup or a secret as data, unchanged", async () => {
    const { sent, source } = recording();
    await applyChanges({ reason: "Sensor stuck",
      source,
      entities: [{ id: ONE.id, changes: [{ attribute: "name", after: "<b>x</b> hunter2" }] }],
    });
    expect(sent[0].attrs).toEqual({ name: { type: "Property", value: "<b>x</b> hunter2" } });
  });
});
