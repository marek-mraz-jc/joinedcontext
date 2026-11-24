// T-7706: DebugPanel's NodeDebug: a rerun alone whose request cannot be made says the test failed with status 0.
import type { JSX } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Trace } from "../src/api/pipelineTest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";

const testPipeline = vi.fn();
vi.mock("../src/api/pipelineTest", async (real) => ({
  ...(await real<typeof import("../src/api/pipelineTest")>()),
  testPipeline: (...args: unknown[]) => testPipeline(...args),
}));

const { toFlow } = await import("../src/pages/pipelines/PipelineFlow");
const { NodeDebug } = await import("../src/pages/pipelines/DebugPanel");

const FORM: PipelineForm = {
  name: "air",
  class: "auto",
  source: { dataSourceRef: "feed" },
  processors: [{ step: { processor: { log: {} } } }],
  compute: { kind: "bloblang", bloblang: 'root = this\nroot.type = "AirQualityObserved"' },
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:ep",
};
const BROKEN: Trace = {
  input: { events: 2, bytes: 40, sample: { id: "s1", pm10: 4 } },
  mapping: [],
  validation: [],
  errors: [{ stage: "mapping", step: 1, message: "expected number, got string" }],
  stages: [
    { step: 0, reached: 2, sample: { id: "s1", pm10: "many" } },
    { step: 1, reached: 2 },
  ],
};
const show = (ui: JSX.Element) => render(<I18nextProvider i18n={i18n}>{ui}</I18nextProvider>);

describe("NodeDebug rerun edges (T-7706)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    testPipeline.mockReset();
  });

  it("says the test failed with status 0 when the rerun cannot reach the server", async () => {
    testPipeline.mockRejectedValue(new TypeError("offline"));
    show(<NodeDebug project="helsinki" form={FORM} trace={BROKEN} nodes={toFlow(FORM).nodes} id="compute" toManifest={(form) => form} />);
    await userEvent.setup().click(screen.getByRole("button", { name: en.pipelines.debug.rerun }));
    expect(await screen.findByText(en.pipelines.test.failed.replace("{status}", "0"))).toBeInTheDocument();
    expect(screen.queryByText(en.pipelines.debug.rerunOk)).toBeNull();
  });
});
