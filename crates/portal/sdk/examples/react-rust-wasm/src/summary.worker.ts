// The WebAssembly module, run in a Web Worker (T-3327): loaded once from the App's own bundle
// (`wasm/pkg`, which the build lane writes before `vite build`), then asked once per question.
import init, { summarize } from "../wasm/pkg/jc_wasm_example.js";
import type { Answer, Ask } from "./summary";

const ready = init();

self.addEventListener("message", (event: MessageEvent<Ask>) => {
  const { id, values } = event.data;
  ready
    .then(() => self.postMessage({ id, summary: Array.from(summarize(values)) } satisfies Answer))
    .catch((error: unknown) => self.postMessage({ id, error: String(error) } satisfies Answer));
});
