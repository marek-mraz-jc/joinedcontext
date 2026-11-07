// A source pipeline keeps writing in every state its device entity can be
// in. Readings expire (4.22), so a page reopened after the retention window
// finds the entity without its attribute: Partial Attribute Update answers
// 404 there and Create Entity answers 409.
import { beforeEach, describe, expect, it, vi } from "vitest";

// A broker reduced to the status codes of the upsert, over one entity.
const entity = { exists: false, hasAttr: false };
vi.mock("../src/broker/api.js", () => ({
  listEntities: vi.fn(async () => []),
  batchUpsert: vi.fn(async (_space, _docs, mode) => {
    if (mode !== "update" && entity.exists) entity.hasAttr = false; // replace drops what is not sent
    const status = entity.exists ? 204 : 201;
    Object.assign(entity, { exists: true, hasAttr: true });
    return { status, ok: true };
  }),
}));
vi.mock("../src/state/board.js", () => ({
  board: { pipes: [], positions: {} },
  burst: vi.fn(),
  emit: vi.fn(),
  refreshSpace: vi.fn(async () => {}),
  save: vi.fn(),
}));

import { tickPipe } from "../src/state/pipes.js";
import { TYPES } from "../src/model.js";

const pipe = () => ({ id: "p1", kind: "source", type: Object.keys(TYPES)[0], into: "s", ticks: 0 });

describe("source pipeline tick", () => {
  beforeEach(() => Object.assign(entity, { exists: false, hasAttr: false }));

  it("creates the device entity on the first tick", async () => {
    const p = pipe();
    await tickPipe(p);
    expect(p.ticks).toBe(1);
    expect(entity).toEqual({ exists: true, hasAttr: true });
  });

  it("updates an entity that still has its reading", async () => {
    Object.assign(entity, { exists: true, hasAttr: true });
    const p = pipe();
    await tickPipe(p);
    expect(p.ticks).toBe(1);
  });

  it("writes again after the reading expired and the entity stayed", async () => {
    Object.assign(entity, { exists: true, hasAttr: false });
    const p = pipe();
    await tickPipe(p);
    expect(p.ticks).toBe(1);
    expect(entity.hasAttr).toBe(true);
  });
});
