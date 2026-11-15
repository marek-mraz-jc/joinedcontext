// Inspection worker running the Rust/WASM module in a background thread.
import init, { inspect } from "../wasm/pkg/data_quality_inspector.js";

const ready = init();

self.onmessage = async (event: MessageEvent<{ id: number; input: string }>) => {
  const { id, input } = event.data;
  try {
    await ready;
    self.postMessage({ id, answer: inspect(input) });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
  }
};
