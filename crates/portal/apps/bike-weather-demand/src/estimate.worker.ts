// The estimation in its own thread (AP-142): the Rust module compiled to WebAssembly, loaded once
// from the bundle's own hashed file, answering each call by the id it came with.
import init, { estimate } from "../wasm/pkg/bike_weather_demand.js";

const ready = init();

self.onmessage = async (event: MessageEvent<{ id: number; input: string }>) => {
  const { id, input } = event.data;
  try {
    await ready;
    self.postMessage({ id, answer: estimate(input) });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
  }
};
