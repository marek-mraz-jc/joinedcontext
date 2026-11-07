/**
 * T-2322: the submit's own state reaches the submit button (UI-01, UI-44, PL-49, T-0962).
 *
 * `SchemaForm` used to hand `submitting` and `submitDisabledReason` to the button through
 * `ui:submitButtonOptions.props`. rjsf copies the uiSchema into its own state and, in a form
 * whose schema has a `required` field, stops re-deriving that state from props once it has
 * validated — so the button was rendered for ever with the options of its first render: it never
 * spun, and it never said why it was closed. Nearly every form of the Portal has a required
 * field, `ResourceFormDialog` included.
 *
 * The pair of cases below is the proof: the same assertions against a schema without a required
 * field (the control, which passed all along) and against one with it (the real case).
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema } from "../src/components/forms/types";
import { expectDenied, expectOpen } from "./checks";

const WITHOUT_REQUIRED: JsonSchema = {
  type: "object",
  properties: { a: { type: "string", title: "A" } },
};

const WITH_REQUIRED: JsonSchema = {
  type: "object",
  required: ["a"],
  properties: { a: { type: "string", title: "A" } },
};

const SCHEMAS: [string, JsonSchema][] = [
  ["without a required field", WITHOUT_REQUIRED],
  ["with a required field", WITH_REQUIRED],
];

function show(schema: JsonSchema, props: { submitting?: boolean; reason?: string } = {}) {
  const onSubmit = vi.fn();
  const view = (next: { submitting?: boolean; reason?: string }) => (
    <I18nextProvider i18n={i18n}>
      <SchemaForm<{ a?: string }>
        schema={schema}
        formData={{ a: "value" }}
        submitLabel="Propose"
        submitting={next.submitting}
        submitDisabledReason={next.reason}
        onSubmit={onSubmit}
      />
    </I18nextProvider>
  );
  const rendered = render(view(props));
  return {
    ...rendered,
    onSubmit,
    /** Re-render with a new state, as a page does when its mutation starts. */
    set: (next: { submitting?: boolean; reason?: string }) => rendered.rerender(view(next)),
  };
}

const submit = () => screen.getByRole("button", { name: "Propose" });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe.each(SCHEMAS)("a form %s", (_label, schema) => {
  it("says_it_is_busy_once_the_submit_is_in_flight", async () => {
    const form = show(schema);
    expect(submit()).not.toHaveAttribute("aria-busy");

    // Validated first: this is the state rjsf stops re-deriving its uiSchema in.
    await userEvent.type(screen.getByLabelText(/A/), "x");
    form.set({ submitting: true });

    await waitFor(() => expect(submit()).toHaveAttribute("aria-busy", "true"));
    expect(submit().querySelector("svg"), "the spinner is drawn, not only announced").not.toBeNull();
  });

  it("says_why_it_is_closed_once_the_gate_closes", async () => {
    const form = show(schema);
    expectOpen(submit());

    await userEvent.type(screen.getByLabelText(/A/), "x");
    form.set({ reason: "Proposing is closed while a change is under review." });

    await waitFor(() =>
      expectDenied(submit(), "Proposing is closed while a change is under review."),
    );
  });

  it("sends_nothing_while_the_gate_is_closed", async () => {
    const form = show(schema);
    await userEvent.type(screen.getByLabelText(/A/), "x");
    form.set({ reason: "Your role may not propose a ContextSpace." });
    await waitFor(() => expectDenied(submit()));

    await userEvent.click(submit());
    expect(form.onSubmit).not.toHaveBeenCalled();
  });

  it("opens_again_when_the_gate_opens", async () => {
    const form = show(schema, { reason: "Closed." });
    expectDenied(submit(), "Closed.");
    form.set({});
    await waitFor(() => expectOpen(submit()));
  });
});
