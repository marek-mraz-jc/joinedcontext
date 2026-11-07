/**
 * T-1843: the model projection picker, against the UI contract (UI-15, UI-16, UI-11, UI-44).
 *
 * `endpoint_form.test.tsx` drives it inside the endpoint dialog and owns what it writes into a
 * manifest. This file mounts it on its own, which is what the survey of 2026-09-18 found
 * missing — no axe run covered it — and holds the controls it used to hand-make: eight bare
 * `<input>`, one bare `<select>` and one arbitrary width, now the shared `Checkbox`, `Input`,
 * `Select` and `Field`, so the focus ring, the disabled state and the label are one style.
 *
 * The names of those controls are the second half of the task: each field was announced by an
 * English word the screen never showed ("Vehicle idPattern") while the person read "ID pattern"
 * in their own language. A name now carries the class and the label as it is written on the
 * screen (WCAG 2.5.3).
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CHECKBOX } from "../src/components/ui/Input";
import { ModelPicker } from "../src/pages/endpoints/ModelPicker";
import type { ModelPickerState } from "../src/pages/endpoints/ModelPicker";
import { expectNoRawKeys } from "./checks";
import { expectNoAxeViolations, inEveryLocale, json, list, renderPart } from "./page_contract";

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

function envelope(kind: string, name: string, spec: Record<string, unknown>) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind, metadata: { name, namespace: PROJECT }, spec };
}

const MODEL = envelope("DataModel", "mobility", { contextSpaceRef: SPACE, version: "3", linkml: LINKML });
const PROJECTION = envelope("ModelProjection", "mobility-public", {
  contextSpaceRef: SPACE,
  dataModelRef: "mobility",
  classes: [{ name: "Vehicle", slots: ["name"] }],
});
const SHARING = envelope("Endpoint", "vehicles-live", {
  contextSpaceRef: SPACE,
  projectionRef: { name: "mobility-public" },
});

/** The endpoint being edited, which already reads that same projection. */
const ITSELF = envelope("Endpoint", "air-public", {
  contextSpaceRef: SPACE,
  projectionRef: { name: "mobility-public" },
});

function state(over: Partial<ModelPickerState> = {}): ModelPickerState {
  return { projectionName: "air-public-projection", classes: {}, ...over };
}

interface Answers {
  models?: unknown[];
  projections?: unknown[];
  endpoints?: unknown[];
}

function answering({ models = [MODEL], projections = [], endpoints = [] }: Answers = {}) {
  return {
    answer: (url: URL) => {
      if (url.pathname.endsWith("/datamodels")) return json(list(models));
      if (url.pathname.endsWith("/projections")) return json(list(projections));
      if (url.pathname.endsWith("/endpoints")) return json(list(endpoints));
      return json(list([]));
    },
  };
}

function picker(props: Partial<Parameters<typeof ModelPicker>[0]> = {}, answers: Answers = {}) {
  const onChange = props.onChange ?? vi.fn();
  const rendered = renderPart(
    <ModelPicker
      project={PROJECT}
      spaceName={SPACE}
      endpointName="air-public"
      value={state()}
      {...props}
      onChange={onChange}
    />,
    answering(answers),
  );
  return { ...rendered, onChange };
}

/** The class tick of `name`, waited for: the model is read before anything is drawn. */
async function classTick(name: string): Promise<HTMLElement> {
  return screen.findByLabelText(name, { selector: "input[type=checkbox]" });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the states the picker has before a class can be ticked", () => {
  it("says a space with no data model has no projection to draw", async () => {
    picker({}, { models: [] });
    expect(await screen.findByText(en.endpoints.picker.noModel)).toBeInTheDocument();
  });

  it("waits with a word while the project's models are being read", async () => {
    picker();
    expect(screen.getByText(en.app.loading)).toBeInTheDocument();
    await classTick("Vehicle");
  });

  // UI-44 as the picker means it: a form that cannot be checked until a class is ticked says so
  // where the ticks are, and offers the one tick a hand-off would have made.
  it("asks for a class while nothing is ticked, and ticks the first on request", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    picker({ onChange });
    await classTick("Vehicle");

    const note = screen.getByRole("note");
    expect(within(note).getByText(en.endpoints.picker.nothingTicked)).toBeInTheDocument();
    await user.click(within(note).getByRole("button", { name: en.form.useExample }));
    expect(onChange).toHaveBeenCalled();
    const written = onChange.mock.calls.at(-1)![0] as ModelPickerState;
    expect(written.classes.Vehicle.ticked).toBe(true);
    // Never more than the first class, and never an identity slot among the chosen ones.
    expect(written.classes.User).toBeUndefined();
    expect(written.classes.Vehicle.slots).toEqual(["name", "speed"]);
  });
});

describe("the ticks and the fields of a class", () => {
  it("names the model's served major version, never its semver (T-3270)", async () => {
    const onChange = vi.fn();
    picker({ onChange }, { models: [{ ...MODEL, spec: { ...MODEL.spec, version: "1.8.0" } }] });
    await classTick("Vehicle");
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect((onChange.mock.calls.at(-1)![0] as ModelPickerState).dataModelVersion).toBe("1");
  });

  it("ticks a class with the shared checkbox, focus ring and all", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    picker({ onChange });

    const vehicle = await classTick("Vehicle");
    expect(vehicle.className).toBe(CHECKBOX);
    await user.click(vehicle);
    expect((onChange.mock.calls.at(-1)![0] as ModelPickerState).classes.Vehicle.ticked).toBe(true);
  });

  // The identity slots are not a choice, and a dimmed tick with nothing said about it is what
  // the file had. The reason is now read with the box.
  it("shows id and type as always exposed, with the reason on the box", async () => {
    picker({ value: state({ classes: { Vehicle: { ticked: true, slots: ["name"] } } }) });

    const id = await screen.findByLabelText("Vehicle.id");
    expect(id).toBeChecked();
    expect(id).toBeDisabled();
    expect(id).toHaveAccessibleDescription(en.endpoints.picker.identityAlways);
    expect(await screen.findByLabelText("Vehicle.type")).toBeDisabled();
    // A slot that is a choice stays one.
    expect(await screen.findByLabelText("Vehicle.speed")).toBeEnabled();
  });

  it("names the three write fields after the class and the label on the screen", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    picker({
      value: state({ classes: { Vehicle: { ticked: true, slots: ["name"], writable: true } } }),
      onChange,
    });

    const pattern = await screen.findByLabelText(`Vehicle ${en.endpoints.picker.idPattern}`);
    expect(await screen.findByLabelText(`Vehicle ${en.endpoints.picker.scope}`)).toBeInTheDocument();
    expect(await screen.findByLabelText(`Vehicle ${en.endpoints.picker.q}`)).toBeInTheDocument();
    expect(await screen.findByLabelText(`Vehicle ${en.endpoints.picker.readQ}`)).toBeInTheDocument();

    // The value is the caller's, so one keystroke is what this asserts: it arrives as a change
    // for that class and nothing else in the state moves.
    await user.type(pattern, "u");
    const written = onChange.mock.calls.at(-1)![0] as ModelPickerState;
    expect(written.classes.Vehicle.idPattern).toBe("u");
    expect(written.classes.Vehicle.slots).toEqual(["name"]);
  });

  it("offers the write fields only for a class that is writable", async () => {
    picker({ value: state({ classes: { Vehicle: { ticked: true, slots: ["name"] } } }) });

    await classTick("Vehicle");
    expect(screen.queryByLabelText(`Vehicle ${en.endpoints.picker.idPattern}`)).toBeNull();
    expect(screen.getByLabelText(`Vehicle ${en.endpoints.picker.writable}`)).not.toBeChecked();
  });

  it("closes every tick and every field while the form is busy", async () => {
    const { container } = picker({
      disabled: true,
      value: state({ classes: { Vehicle: { ticked: true, slots: ["name"], writable: true } } }),
    });

    await classTick("Vehicle");
    for (const control of container.querySelectorAll<HTMLElement>("input, select")) {
      expect(control, control.getAttribute("aria-label") ?? control.id).toBeDisabled();
    }
  });
});

describe("a projection the endpoint shares with another", () => {
  it("is read-only, says who else reads it, and offers a copy of its own", async () => {
    picker(
      { value: state({ selectedProjectionRef: "mobility-public" }) },
      { projections: [PROJECTION], endpoints: [SHARING, ITSELF] },
    );

    expect(
      await screen.findByText(
        en.endpoints.picker.sharedBy.replace("{endpoints}", "vehicles-live"),
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.endpoints.picker.detach })).toBeInTheDocument();
    // And never about itself: the endpoint being edited reads that projection too.
    expect(screen.queryByText(/air-public/)).toBeNull();
    // The name of a shared projection is not this endpoint's to change.
    expect(screen.queryByLabelText(en.endpoints.picker.projectionName)).toBeNull();
    expect(await screen.findByLabelText("Vehicle")).toBeDisabled();
  });

  it("names its own projection in a labelled field while it draws a new one", async () => {
    picker({}, { projections: [PROJECTION] });

    const name = await screen.findByLabelText(en.endpoints.picker.projectionName);
    expect(name).toHaveValue("air-public-projection");
    const reuse = screen.getByLabelText(en.endpoints.picker.reuse);
    expect(reuse.tagName).toBe("SELECT");
    expect(within(reuse).getByRole("option", { name: "mobility-public" })).toBeInTheDocument();
  });
});

describe("the picker as a screen reader and a translator find it", () => {
  it("has no axe violations with a writable class open", async () => {
    const { container } = picker({
      value: state({ classes: { Vehicle: { ticked: true, slots: ["name"], writable: true } } }),
    });

    await classTick("Vehicle");
    await expectNoAxeViolations(container);
  });

  it("draws every string of its own in all four locales", async () => {
    await inEveryLocale(async (locale) => {
      const { container, unmount } = picker({
        value: state({ classes: { Vehicle: { ticked: true, slots: ["name"], writable: true } } }),
      });
      await classTick("Vehicle");
      expectNoRawKeys(container);
      await waitFor(() =>
        expect(
          screen.getByLabelText(`Vehicle ${i18n.t("endpoints.picker.writable")}`),
          `${locale} names the writable tick in its own language`,
        ).toBeInTheDocument(),
      );
      unmount();
    });
  });
});
