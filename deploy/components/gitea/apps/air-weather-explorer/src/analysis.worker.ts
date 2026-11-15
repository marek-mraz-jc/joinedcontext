/// <reference lib="webworker" />
import { analyseHere } from "./analysis";
import type { AnalysisInput } from "./analysis";

// The WebAssembly statistics, off the page's thread (T-3330): one message in, one result or one error out.
self.onmessage = async (event: MessageEvent<{ id: number; input: AnalysisInput }>) => {
  const { id, input } = event.data;
  try {
    self.postMessage({ id, result: await analyseHere(input) });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
  }
};
