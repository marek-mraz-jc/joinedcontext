// jsdom ships no types and @types/jsdom trails its major version: the part the chart view test
// uses (tests/chartView.test.ts).
declare module "jsdom" {
  export class JSDOM {
    constructor(html: string, options?: { runScripts?: "dangerously" | "outside-only" });
    readonly window: Window & typeof globalThis;
  }
}
