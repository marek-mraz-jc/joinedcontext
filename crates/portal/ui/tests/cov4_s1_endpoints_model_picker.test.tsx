// T-7706: ModelPicker gaps: a hand-off ticking classes, drawing a new projection again, unticking a class, the projection name, the read and write queries of a writable class.
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ModelPicker } from "../src/pages/endpoints/ModelPicker";
import type { ModelPickerState } from "../src/pages/endpoints/ModelPicker";
import { json, list, renderPart } from "./page_contract";

const PROJECT = "banskabystrica";
const SPACE = "ovzdusie";

const LINKML = `
id: https://hel.fi/models/mobility
name: mobility
classes:
  Vehicle:
    slots: [id, type, name, speed]
  User:
    slots: [id, type, email]
slots:
  id: { range: string }
  type: { range: string }
  name: { range: string }
  speed: { range: integer }
  email: { range: string }
`;

const envelope = (kind: string, name: string, spec: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
});
const MODEL = envelope("DataModel", "mobility", { contextSpaceRef: SPACE, version: "3", linkml: LINKML });
const PROJECTION = envelope("ModelProjection", "mobility-public", {
  contextSpaceRef: SPACE,
  dataModelRef: "mobility",
  classes: [{ name: "Vehicle", slots: ["name"] }],
});

const state = (over: Partial<ModelPickerState> = {}): ModelPickerState => ({ projectionName: "air-public-projection", classes: {}, ...over });

function picker(props: Partial<Omit<Parameters<typeof ModelPicker>[0], "onChange">> = {}, projections: unknown[] = []) {
  const onChange = vi.fn<(next: ModelPickerState) => void>();
  renderPart(
    <ModelPicker project={PROJECT} spaceName={SPACE} endpointName="air-public" value={state()} {...props} onChange={onChange} />,
    {
      answer: (url: URL) => {
        if (url.pathname.endsWith("/datamodels")) return json(list([MODEL]));
        if (url.pathname.endsWith("/projections")) return json(list(projections));
        return json(list([]));
      },
    },
  );
  return onChange;
}

const last = (onChange: ReturnType<typeof picker>) => onChange.mock.calls.at(-1)![0];

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("a hand-off into the picker", () => {
  it("ticks only the classes the hand-off names, without their identity slots", async () => {
    const onChange = picker({ handed: ["Vehicle"] });
    await waitFor(() => expect(last(onChange).classes.Vehicle?.ticked).toBe(true));
    expect(last(onChange).classes.Vehicle.slots).toEqual(["name", "speed"]);
    expect(last(onChange).classes.User).toBeUndefined();
  });

  it("ticks every class when the hand-off names none", async () => {
    const onChange = picker({ handed: [] });
    await waitFor(() => expect(Object.keys(last(onChange).classes).sort()).toEqual(["User", "Vehicle"]));
  });

  it("ticks nothing when the hand-off names a class the model does not hold", async () => {
    const onChange = picker({ handed: ["Ghost"] });
    await screen.findByLabelText("Vehicle", { selector: "input[type=checkbox]" });
    expect(onChange.mock.calls.every(([next]) => Object.keys(next.classes).length === 0)).toBe(true);
  });
});

describe("a hand-off over a form that already has a ticked class", () => {
  it("leaves the ticks the person made", async () => {
    const onChange = picker({ handed: ["User"], value: state({ classes: { Vehicle: { ticked: true, slots: [] } } }) });
    await screen.findByLabelText("Vehicle", { selector: "input[type=checkbox]" });
    expect(onChange.mock.calls.every(([next]) => next.classes.User === undefined)).toBe(true);
  });
});

describe("what a person edits in the picker", () => {
  it("goes back to drawing a new projection from a reused one", async () => {
    const user = userEvent.setup();
    const onChange = picker({ value: state({ selectedProjectionRef: "mobility-public", projectionName: "mobility-public" }) }, [PROJECTION]);
    const select = await screen.findByLabelText(en.endpoints.picker.reuse);
    await user.selectOptions(select, "");
    expect(last(onChange).selectedProjectionRef).toBeUndefined();
    expect(last(onChange).projectionName).toBe("mobility-public");
  });

  it("names the new projection by what is typed", async () => {
    const user = userEvent.setup();
    const onChange = picker();
    await user.type(await screen.findByLabelText(new RegExp(en.endpoints.picker.projectionName)), "x");
    expect(last(onChange).projectionName).toBe("air-public-projectionx");
  });

  it("unticks a ticked class and drops its settings", async () => {
    const user = userEvent.setup();
    const onChange = picker({ value: state({ classes: { Vehicle: { ticked: true, slots: ["name"] } } }) });
    await user.click(await screen.findByLabelText("Vehicle", { selector: "input[type=checkbox]" }));
    expect(last(onChange).classes.Vehicle).toBeUndefined();
  });

  it("takes the read and write queries of a writable class", async () => {
    const user = userEvent.setup();
    const onChange = picker({ value: state({ classes: { Vehicle: { ticked: true, slots: ["name"], writable: true } } }) });
    await user.type(await screen.findByLabelText(`Vehicle ${en.endpoints.picker.readQ}`), "r");
    expect(last(onChange).classes.Vehicle.readQ).toBe("r");
    await user.type(screen.getByLabelText(`Vehicle ${en.endpoints.picker.q}`), "w");
    expect(last(onChange).classes.Vehicle.writeQ).toBe("w");
  });
});
