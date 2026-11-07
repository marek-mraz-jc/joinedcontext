/**
 * T-4845: per attribute a plain history choice in the workbench's Target card, saved as
 * `spec.history` (PL-106). Each choice goes form -> manifest -> form unchanged, the server's
 * history findings show beside the attribute they name, and the column never shows a value.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { TargetCard } from "../src/pages/pipelines/WorkbenchTarget";
import type { TargetType } from "../src/pages/pipelines/WorkbenchTarget";
import { planOf, timeSlotsOf, withPlan, type History } from "../src/pages/pipelines/history";
import { fromManifest, toEnvelope as pipelineEnvelope } from "../src/pages/pipelines/PipelineEditor";

const TARGET: TargetType = {
  model: { name: "helsinki", version: "v1" },
  attributes: [
    { name: "location", kind: "GeoProperty", valueType: "object", required: false },
    { name: "speed", kind: "Property", valueType: "number", required: false, unit: { code: "MTS" } },
    { name: "observedOn", kind: "Property", valueType: "string", format: "date-time", required: false },
    { name: "nextStop", kind: "Relationship", required: false, relationship: { target: "Stop", many: false } },
  ],
  writable: [
    { name: "location", writable: true },
    { name: "speed", writable: true },
    { name: "observedOn", writable: true },
    { name: "nextStop", writable: false, reason: "not writable through helsinki-all" },
  ],
  loading: false,
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

const inI18n = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

describe("the history plan of one attribute", () => {
  it("writes each choice into the manifest and reads it back as made", () => {
    const plans = [
      { keep: "every" as const, observedAt: "observedOn" },
      { keep: "changes" as const, heartbeat: "PT15M", observedAt: "observedOn" },
      { keep: "latest" as const },
      { keep: "every" as const, observedAt: "received" },
    ];
    for (const plan of plans) {
      const history = withPlan(undefined, "Vehicle", "location", plan);
      expect(history).toEqual([{ type: "Vehicle", attributes: [{ names: ["location"], ...plan }] }]);
      expect(planOf(history, "Vehicle", "location")).toEqual(plan);
      const stored = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Pipeline", metadata: { name: "hfp", namespace: "helsinki" }, spec: { class: "resident", history } };
      const form = fromManifest(stored);
      expect(form.history).toEqual(history);
      expect(pipelineEnvelope("helsinki", form, stored).spec.history).toEqual(history);
    }
  });

  it("gives keep only changes a heartbeat and drops one the other choices do not take", () => {
    expect(withPlan(undefined, "Vehicle", "speed", { keep: "changes" })).toEqual([{ type: "Vehicle", attributes: [{ names: ["speed"], keep: "changes", heartbeat: "PT1H" }] }]);
    expect(withPlan(undefined, "Vehicle", "speed", { keep: "latest", heartbeat: "PT1H" })).toEqual([{ type: "Vehicle", attributes: [{ names: ["speed"], keep: "latest" }] }]);
  });

  it("moves one name out of a shared entry, keeps the others and leaves no empty plan", () => {
    const shared: History[] = [
      { type: "Vehicle", attributes: [{ names: ["location", "speed"], keep: "changes", heartbeat: "PT15M" }] },
      { type: "Stop", attributes: [{ names: ["name"], keep: "latest" }] },
    ];
    const moved = withPlan(shared, "Vehicle", "speed", { keep: "latest" });
    expect(planOf(moved, "Vehicle", "location")).toEqual({ keep: "changes", heartbeat: "PT15M" });
    expect(planOf(moved, "Vehicle", "speed")).toEqual({ keep: "latest" });
    expect(planOf(moved, "Stop", "name")).toEqual({ keep: "latest" });
    const cleared = withPlan(withPlan(moved, "Vehicle", "speed", undefined), "Vehicle", "location", undefined);
    expect(cleared).toEqual([{ type: "Stop", attributes: [{ names: ["name"], keep: "latest" }] }]);
    expect(withPlan(cleared, "Stop", "name", undefined)).toBeUndefined();
    const stored = { apiVersion: "joinedcontext.com/v1alpha1", kind: "Pipeline", metadata: { name: "hfp", namespace: "helsinki" }, spec: { class: "resident", history: shared } };
    expect(pipelineEnvelope("helsinki", { ...fromManifest(stored), history: undefined }, stored).spec.history).toBeUndefined();
  });

  it("offers the time slots the sample records carry, or every one before a sample exists", () => {
    expect(timeSlotsOf(["observedOn", "validFrom"], [])).toEqual(["observedOn", "validFrom"]);
    expect(timeSlotsOf(["observedOn", "validFrom"], [{ id: "urn:ngsi-ld:Vehicle:1", observedOn: { value: "2026-10-20T10:00:00Z" } }])).toEqual(["observedOn"]);
  });
});

describe("the History column", () => {
  it("sets a plan from the selects and names the time slot picked from the sample", () => {
    const onHistory = vi.fn();
    inI18n(<TargetCard space="helsinki" type="Vehicle" target={TARGET} history={undefined} onHistory={onHistory} timeSlots={["observedOn"]} />);
    const table = screen.getByRole("table", { name: "Attributes of Vehicle" });
    expect(within(table).getByRole("columnheader", { name: "History" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "What to keep of location" }), { target: { value: "changes" } });
    expect(onHistory).toHaveBeenLastCalledWith([{ type: "Vehicle", attributes: [{ names: ["location"], keep: "changes", heartbeat: "PT1H" }] }]);
    expect(screen.queryByRole("combobox", { name: "What to keep of nextStop" })).toBeNull();
  });

  it("shows the time and heartbeat of a plan, the refusal beside its attribute and no value", () => {
    const history: History[] = [
      { type: "Vehicle", attributes: [
        { names: ["location"], keep: "changes", heartbeat: "PT15M", observedAt: "observedOn" },
        { names: ["speed"], keep: "latest" },
      ] },
    ];
    const findings = [{ source: "history", rule: "latest", field: "speed", reason: "keep the latest value only needs Vehicle's history off, and space helsinki keeps P30D of it" }];
    inI18n(<TargetCard space="helsinki" type="Vehicle" target={TARGET} history={history} onHistory={() => {}} timeSlots={["observedOn"]} historyFindings={findings} />);
    const row = (name: string) => within(screen.getByRole("table", { name: "Attributes of Vehicle" })).getAllByRole("row").find((r) => within(r).queryByText(name, { selector: "span.font-mono" })) as HTMLElement;
    expect(within(row("location")).getByRole("combobox", { name: "Time of a row of location" })).toHaveValue("observedOn");
    expect(within(row("location")).getByRole("combobox", { name: "Row at least every, for location" })).toHaveValue("PT15M");
    expect(row("location")).toHaveTextContent("kept as chosen");
    expect(row("speed")).toHaveTextContent("cannot be kept");
    expect(row("speed")).toHaveTextContent("space helsinki keeps P30D");
    expect(within(row("speed")).queryByRole("combobox", { name: "Time of a row of speed" })).toBeInTheDocument();
    expect(within(row("speed")).queryByRole("combobox", { name: "Row at least every, for speed" })).toBeNull();
  });

  it("is absent where the card edits no plan", () => {
    inI18n(<TargetCard space="helsinki" type="Vehicle" target={TARGET} />);
    expect(screen.queryByRole("columnheader", { name: "History" })).toBeNull();
  });
});
