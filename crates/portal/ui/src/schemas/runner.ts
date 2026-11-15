import components from "./ekiben-components.json";

/** The inputs and processors the pipeline runner (ekiben) builds, written from its registry by
 * crates/ekiben/crates/ekiben-server/tests/portal_catalog.rs (T-6825). The Bento catalogs draw
 * the forms; these lists keep the pickers to what runs. The server checks the same lists on save. */
export const RUNNER_INPUTS: ReadonlySet<string> = new Set(components.inputs);
export const RUNNER_PROCESSORS: ReadonlySet<string> = new Set(components.processors);

/** The runner inputs the platform refuses (jc-core `REFUSED_INPUTS`, PL-50, T-3833): they read
 * the runner's own process or another stream. tests/runner_lists.test.ts keeps it equal. */
export const REFUSED_INPUTS: ReadonlySet<string> = new Set([
  "dynamic",
  "http_server",
  "inproc",
  "kubernetes_watch",
  "resource",
  "socket_server",
  "stdin",
  "subprocess",
]);

/** The processors the platform refuses as an author step (jc-core `REFUSED_PROCESSORS`, PL-52):
 * the runner may build one (`javascript` runs only from a reviewed Script), a step never names it.
 * tests/runner_lists.test.ts keeps it equal. */
export const REFUSED_PROCESSORS: ReadonlySet<string> = new Set([
  "awk",
  "command",
  "file",
  "javascript",
  "python",
  "subprocess",
  "template",
  "wasm",
]);

/** Whether a pipeline step may name this processor: the runner builds it and it is not refused. */
export const offeredProcessor = (name: string): boolean =>
  RUNNER_PROCESSORS.has(name) && !REFUSED_PROCESSORS.has(name);

/** Whether a DataSource may name this runner input: the runner builds it and it is not refused. */
export const offeredInput = (name: string): boolean => RUNNER_INPUTS.has(name) && !REFUSED_INPUTS.has(name);
