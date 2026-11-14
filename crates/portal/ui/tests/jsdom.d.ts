// jsdom ships no types and @types/jsdom trails its major version: the part the chart view test
// uses (tests/chartView.test.ts).
declare module "jsdom" {
  export class VirtualConsole {
    on(event: "jsdomError", listener: (error: Error) => void): this;
  }
  export class JSDOM {
    constructor(
      html: string,
      options?: {
        runScripts?: "dangerously" | "outside-only";
        pretendToBeVisual?: boolean;
        virtualConsole?: VirtualConsole;
        beforeParse?: (window: Window & typeof globalThis) => void;
      },
    );
    readonly window: Window & typeof globalThis;
  }
}
