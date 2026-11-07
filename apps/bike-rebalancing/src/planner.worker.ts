/// <reference lib="webworker" />
import { planHere } from "./planner";
import type { PlanInput } from "./planner";

// The WebAssembly planner, off the page's thread (T-3328): one message in, one plan or one error out.
self.onmessage = async (event: MessageEvent<{ id: number; input: PlanInput }>) => {
  const { id, input } = event.data;
  try {
    self.postMessage({ id, plan: await planHere(input) });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
  }
};
