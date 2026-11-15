/// <reference lib="webworker" />
import { planHere } from "./planner";
import type { PlanInput } from "./planner";

// The WebAssembly day planner, off the page's thread (T-3329): one message in, one day or one error out.
self.onmessage = async (event: MessageEvent<{ id: number; input: PlanInput }>) => {
  const { id, input } = event.data;
  try {
    self.postMessage({ id, day: await planHere(input) });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
  }
};
