import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Plan, PlanInput } from "./planner";

/** A stand-in for the browser's Worker: it keeps what the page posts and answers when told to. */
class FakeWorker {
  static made: FakeWorker[] = [];
  onmessage: ((event: { data: { id: number; plan?: Plan; error?: string } }) => void) | null = null;
  onerror: (() => void) | null = null;
  posted: Array<{ id: number; input: PlanInput }> = [];
  constructor() {
    FakeWorker.made.push(this);
  }
  postMessage(message: { id: number; input: PlanInput }) {
    this.posted.push(message);
  }
  answer(data: { id: number; plan?: Plan; error?: string }) {
    this.onmessage?.({ data });
  }
}

const INPUT: PlanInput = { stations: [], settings: { vanCapacity: 5 } };
const PLAN = { needs: [], route: { stops: [], moved: 0, km: 0 } } as unknown as Plan;

// The planner's module keeps its one worker, so each case loads it afresh.
async function planner() {
  vi.resetModules();
  return import("./planner");
}

beforeEach(() => {
  FakeWorker.made = [];
  vi.stubGlobal("Worker", FakeWorker);
});
afterEach(() => vi.unstubAllGlobals());

describe("the planner off the page's thread", () => {
  it("posts each plan to one worker and resolves each with its own answer, ignoring a stray one", async () => {
    const { computePlan } = await planner();
    const first = computePlan(INPUT);
    const second = computePlan(INPUT);
    expect(FakeWorker.made).toHaveLength(1);
    const [worker] = FakeWorker.made;
    const [a, b] = worker.posted.map((message) => message.id);
    worker.answer({ id: 999, plan: PLAN });
    worker.answer({ id: b, plan: PLAN });
    worker.answer({ id: a, error: "no stations" });
    await expect(second).resolves.toBe(PLAN);
    await expect(first).rejects.toThrow("no stations");
  });

  it("says the planner did not answer when the worker sends neither a plan nor an error", async () => {
    const { computePlan } = await planner();
    const asked = computePlan(INPUT);
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id });
    await expect(asked).rejects.toThrow("The planner did not answer.");
  });

  it("fails every waiting plan when the worker stops, and starts a new worker for the next", async () => {
    const { computePlan } = await planner();
    const waiting = [computePlan(INPUT), computePlan(INPUT)];
    FakeWorker.made[0].onerror?.();
    for (const one of waiting) await expect(one).rejects.toThrow("The planner stopped. Reload the page.");
    void computePlan(INPUT);
    expect(FakeWorker.made).toHaveLength(2);
  });
});
