/**
 * T-1762: the indicator card's keep-updated form against the UI contract (UI-04, UI-11, UI-15, UI-16, UI-48).
 *
 * The defect this file holds is a translation one, and it was visible to everybody but an
 * English reader: three fragments of the message that asks the assistant to keep an indicator
 * updated were built in code in English — `count of X`, `avg of pm10 over AirQualityObserved`,
 * ` where pm10>0` — and interpolated into a sentence the bundle translates. A Czech steward
 * pressing "keep it updated" sent the agent a half-Czech instruction and then read it back in
 * their own transcript.
 *
 * The other two findings of this task (hand-rolled radios, a bare input outside `Field`) were
 * fixed by T-2408 before this batch: the form is a `RadioGroup` with a legend and two `Field`s
 * today, and `checkForm` below is what keeps it that way.
 */
import { screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { checkForm, type FormSpec } from "./formContract";
import { LOCALES } from "./page_contract";
import { keepMessage } from "../src/pages/apps/KpiCard";
import type { Kpi } from "../src/pages/apps/KpiCard";

const KPI: Kpi = {
  name: "average-pm10",
  title: "Average PM10",
  value: 18.4,
  unit: "GQ",
  formula: "avg(pm10) over AirQualityObserved",
  count: 12,
  space: "helsinki-kpi",
  endpointSlug: "kpislug0000000000000000000",
  endpointName: "helsinki-kpi-all",
  entity: { id: "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:average-pm10" },
  query: { type: "AirQualityObserved", attribute: "pm10", agg: "avg", q: "pm10>0" },
};

const { KpiCard } = await import("../src/pages/apps/KpiCard");

const spec: FormSpec = {
  fields: [
    { label: /minutes/, value: "30" },
    { label: /Into the space/, value: "air-kpi" },
  ],
  submit: /Draft the pipeline/,
  // The form hands its message to the conversation; nothing is posted.
  path: "/projects/helsinki",
  answer: () => undefined,
  open: async (user) => {
    await user.click(screen.getByRole("button", { name: /Keep it updated/ }));
  },
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the keep-updated form of the indicator card (T-1762)", () => {
  it("meets_the_form_contract", async () => {
    await checkForm(
      (proposed) => <KpiCard project="helsinki" kpi={KPI} onSend={proposed} />,
      spec,
    );
  });

  it.each(LOCALES)("writes_the_whole_message_in_%s", async (locale) => {
    await i18n.changeLanguage(locale);
    const t = i18n.getFixedT(locale);
    const message = keepMessage(KPI, { onChange: false, minutes: 15, space: "air-kpi" }, t);
    // Every word of the sentence is the bundle's: the aggregation, what it is over, the filter
    // and the frame around them. What stays as it is: the names the person chose — the type,
    // the attribute, the query and the spaces.
    expect(message).toContain(t("agentRun.kpi.overAgg", { agg: "avg", attribute: "pm10", type: "AirQualityObserved" }));
    expect(message).toContain("air-kpi");
    if (locale !== "en") {
      expect(message, "an English fragment was built into a translated sentence").not.toMatch(
        / of | over | where /,
      );
    }
  });

  it("names_a_count_without_an_attribute_and_keeps_the_filter", async () => {
    const t = i18n.getFixedT("en");
    const counted: Kpi = { ...KPI, query: { type: "BikeHireDockingStation", attribute: "", agg: "count" } };
    expect(keepMessage(counted, { onChange: false, minutes: 15, space: "transportation-kpi" }, t)).toContain(
      "count of BikeHireDockingStation",
    );
    expect(keepMessage(KPI, { onChange: true, minutes: 15, space: "air-kpi" }, t)).toContain(
      "avg of pm10 over AirQualityObserved where pm10>0",
    );
  });

  it("falls_back_to_the_formula_when_the_indicator_carries_no_query", async () => {
    const t = i18n.getFixedT("en");
    const noQuery: Kpi = { ...KPI, query: undefined };
    expect(keepMessage(noQuery, { onChange: false, minutes: 15, space: "air-kpi" }, t)).toContain(
      "avg(pm10) over AirQualityObserved",
    );
  });
});
