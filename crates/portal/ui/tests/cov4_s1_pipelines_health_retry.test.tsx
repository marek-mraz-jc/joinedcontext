// T-7706: PipelineHealth: a refused health read offers Retry, and Retry reads it again.
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";

const PATH = "/projects/helsinki/pipelines";
const pipeline = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "podujatia-detva", namespace: "helsinki" },
  spec: { class: "scheduled" },
  status: { phase: "Live" },
};

describe("the source health tab when its read fails", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads again on Retry and then lists the source", async () => {
    let reads = 0;
    await renderRoute({
      path: PATH,
      answer: (path) => {
        if (path.endsWith("/pipeline-health")) {
          reads += 1;
          return reads === 1
            ? problem(503, "the run store is restarting")
            : jsonResponse({ items: [{ pipeline: "podujatia-detva", state: "ok", failedRuns: 0, alert: false }] });
        }
        if (path.endsWith("/pipelines")) return jsonResponse(list([pipeline]));
        return undefined;
      },
    });
    await userEvent.click(await screen.findByRole("tab", { name: en.pipelines.map.view.health }));
    expect(await screen.findByText(/the run store is restarting/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(reads).toBe(2));
    expect(await screen.findByRole("table", { name: en.pipelines.health.caption })).toBeInTheDocument();
  });
});
