/**
 * T-1848: the model file drop against the UI contract (UI-01, UI-15, UI-16, UI-44, UI-48).
 *
 * The drop wrote its own `<input type="file">` twice over, with one focus ring on the paperclip
 * and none on the button; the preview's refusals greyed "Populate the editor" with no reason a
 * screen reader could reach; and the paperclip's failure message was a hand-styled box that
 * looked like nothing else in the Portal. The input is the shared `FilePicker` now, the refusal
 * is the button's own `disabledReason`, and the message is an `Alert`.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/FilePicker.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied, expectNoRawKeys, expectNoViolations } from "./checks";
import { ModelFileDrop } from "../src/pages/models/ModelFileDrop";
import type { InferAnswer } from "../src/pages/models/ModelFileDrop";

const ANSWER: InferAnswer = {
  linkml: "name: sensors\n",
  operations: [
    { op: "addClass", name: "Sensors", is_a: "Entity" },
    { op: "addSlot", name: "temp", class: "Sensors", range: "float" },
  ],
  detectedTypes: { temp: "float" },
  matches: {},
  untyped: [],
  rows: 2,
};

/** An answer whose second operation the editor cannot apply: the preview must refuse it. */
const REFUSED: InferAnswer = {
  ...ANSWER,
  operations: [
    { op: "addClass", name: "Sensors", is_a: "Entity" },
    { op: "addSlot", name: "temp", class: "NoSuchClass", range: "float" },
  ],
};

function csv(name = "sensors.csv"): File {
  return new File(["id,temp\n1,21.5\n"], name, { type: "text/csv" });
}

function urlOf(input: unknown): string {
  return typeof input === "string" ? input : input instanceof Request ? input.url : String(input);
}

function show(answer: InferAnswer | number, props: { icon?: boolean; onTable?: (file: File) => void } = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) =>
    urlOf(input).includes("/organizations")
      ? Promise.resolve(
          new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        )
      : Promise.resolve(
          typeof answer === "number"
            ? new Response(JSON.stringify({ detail: "the sample is not a table" }), {
                status: answer,
                headers: { "Content-Type": "application/json" },
              })
            : new Response(JSON.stringify(answer), {
                status: 200,
                headers: { "Content-Type": "application/json" },
              }),
        ),
  );
  vi.stubGlobal("fetch", fetchMock);
  const onPopulate = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ModelFileDrop project="helsinki" onPopulate={onPopulate} {...props} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, onPopulate, container: view.container, user: userEvent.setup() };
}

const chooser = () => screen.getByLabelText(en.models.infer.chooseFile);

describe("the model file drop against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-1; Secure; path=/";
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("puts the file input in the tab order with a name of its own", () => {
    const { container } = show(ANSWER);
    const input = chooser();
    input.focus();
    expect(input).toHaveFocus();
    expect(input).not.toBeDisabled();
    // The ring is drawn on the label, because the input itself is what a person cannot see.
    expect(input.closest("label")).toHaveClass("focus-ring-within");
    expect(container.querySelector("input[type=file]")).toBe(input);
  });

  it("takes the same file twice, because the input lets go of it between choices", async () => {
    const { fetchMock } = show(ANSWER);
    await userEvent.upload(chooser(), csv());
    await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button", { name: en.models.infer.cancel }));

    await userEvent.upload(chooser(), csv());
    await screen.findByRole("dialog");
    expect(fetchMock.mock.calls.filter((call) => urlOf(call[0]).includes("/tools/infer-schema"))).toHaveLength(2);
  });

  it("refuses Populate with the count of refusals, and keeps the button reachable", async () => {
    show(REFUSED);
    await userEvent.upload(chooser(), csv());
    const dialog = await screen.findByRole("dialog");
    const populate = within(dialog).getByRole("button", { name: en.models.infer.populate });
    expectDenied(populate, /1/);
  });

  it("says what went wrong in the shared alert, in the paperclip form too", async () => {
    show(422, { icon: true });
    await userEvent.upload(screen.getByLabelText(en.assistant.attach), csv());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("the sample is not a table");
  });

  it("gives a table to onTable without sending it, and still drafts from a JSON sample", async () => {
    const onTable = vi.fn();
    const { fetchMock } = show(ANSWER, { icon: true, onTable });
    const file = csv();
    await userEvent.upload(screen.getByLabelText(en.assistant.attach), file);
    expect(onTable).toHaveBeenCalledWith(file);
    expect(fetchMock.mock.calls.filter(([input]) => urlOf(input).includes("infer-schema"))).toHaveLength(0);
    await userEvent.upload(screen.getByLabelText(en.assistant.attach), new File(["{}"], "sample.json"));
    await screen.findByRole("dialog");
    expect(onTable).toHaveBeenCalledTimes(1);
  });

  it("has no axe violation, closed and with the preview open", async () => {
    const { container } = show(ANSWER);
    await expectNoViolations(container);
    await userEvent.upload(chooser(), csv());
    await screen.findByRole("dialog");
    await expectNoViolations(document.body);
  });

  it("shows no raw translation key in any locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container } = show(ANSWER);
      expectNoRawKeys(container);
      vi.unstubAllGlobals();
    }
    await i18n.changeLanguage("en");
  });
});
