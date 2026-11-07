// The analyser of the tests: the same module the worker loads, compiled for the browser, run
// in-process (jsdom has no Worker). Vite inlines the module's bytes for the test, so no file is
// read and no network is asked. `pnpm wasm` (or the lane's build-wasm) writes wasm/pkg first.
import wasm from "../wasm/pkg/kpi_forecast_bg.wasm?url&inline";
import { analyse, initSync } from "../wasm/pkg/kpi_forecast.js";
import { parseAnswer } from "./analysis";
import type { Analyser } from "./analysis";

const base64 = wasm.slice(wasm.indexOf(",") + 1);
initSync({ module: Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)) });

export const inProcess: Analyser = async (input) => parseAnswer(analyse(JSON.stringify(input)));
