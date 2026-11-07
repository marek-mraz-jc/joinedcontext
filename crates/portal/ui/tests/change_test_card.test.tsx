/**
 * The test of a change the assistant opened (T-0737, AG-77, PL-45, MF-39): a pipeline tested on a
 * fetch of its data source with the entities it mapped, a data source's URL that did not answer,
 * and a pipeline that could not be tested, each read only from what the Portal wrote.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ChangeTestCard, changeTestOf } from "../src/pages/apps/ChangeTestCard";

const step = (status: string, test: unknown) => ({
  tool: "change_resource",
  status,
  output: { kind: "Pipeline", name: "hel-bikes", checked: true, test },
});

const TESTED = step("ok", {
  source: { dataSource: "hsl-bikes" },
  verdict: { ok: true, findings: [] },
  records: 2,
  sample: [
    { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001", type: "BikeHireDockingStation", availableBikeNumber: 3 },
    { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002", type: "BikeHireDockingStation", availableBikeNumber: "<b>0</b>" },
  ],
});

function renderCard(payload: Record<string, unknown>) {
  const test = changeTestOf(payload);
  expect(test).not.toBeNull();
  render(
    <I18nextProvider i18n={i18n}>
      <ChangeTestCard test={test!} />
    </I18nextProvider>,
  );
  return screen.getByRole("region", { name: en.agentRun.changeTest.title });
}

describe("the change's test", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("reads a test only from a change_resource step that carries one", () => {
    expect(changeTestOf({ tool: "change_resource", status: "ok", output: { kind: "Endpoint", checked: true } })).toBeNull();
    expect(changeTestOf({ tool: "draft_kpi_pipeline", output: { test: {} } })).toBeNull();
    expect(changeTestOf(TESTED)?.outcome).toBe("passed");
  });

  it("shows a pipeline tested on its data source with the entities it mapped, drawn as text", () => {
    const card = renderCard(TESTED);
    expect(within(card).getByText(en.agentRun.changeTest.passed)).toBeInTheDocument();
    expect(within(card).getByText("Ran on one fetch of the data source hsl-bikes")).toBeInTheDocument();
    expect(within(card).getByText("2 records")).toBeInTheDocument();
    const table = within(card).getByRole("table");
    expect(within(table).getByText("001")).toBeInTheDocument();
    expect(within(table).getByText("<b>0</b>")).toBeInTheDocument();
  });

  it("shows why a data source's new URL did not pass", () => {
    const card = renderCard({
      tool: "change_resource",
      status: "failed",
      output: {
        kind: "DataSource",
        name: "hsl-bikes",
        test: { source: { url: "https://feeds.example/moved.json" }, probe: { skipped: "the feed answered 404 Not Found" } },
      },
    });
    expect(within(card).getByText(en.agentRun.changeTest.failed)).toBeInTheDocument();
    expect(within(card).getByText("Fetched https://feeds.example/moved.json once")).toBeInTheDocument();
    expect(within(card).getByText("the feed answered 404 Not Found")).toBeInTheDocument();
    expect(within(card).queryByRole("table")).not.toBeInTheDocument();
  });

  it("says why a pipeline could not be tested and shows no records", () => {
    const card = renderCard(step("ok", { untested: "this conversation does not read its endpoint 'helsinki-all'" }));
    expect(within(card).getByText(en.agentRun.changeTest.untested)).toBeInTheDocument();
    expect(within(card).getByText("this conversation does not read its endpoint 'helsinki-all'")).toBeInTheDocument();
    expect(within(card).queryByText(/records?$/)).not.toBeInTheDocument();
  });
});
