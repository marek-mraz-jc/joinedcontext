/**
 * T-1757: SchemaForm against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * Read line by line against the contract and found clean: every chrome element comes from
 * `portalTemplates`/`portalThemeWidgets`, the one classed element uses two tokens, the submit's
 * reason becomes a `<span role="status">` beside the button, `liveValidate` with
 * `showErrorList={false}` puts each message on its own field, and every message is translated
 * through `transformErrors`. This file is what keeps it that way: the form it wraps is the one
 * every schema-driven kind of the Portal is written in, so a regression here is a regression
 * everywhere.
 *
 * `checkForm` is T-1730's and joins this file with it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { ErrorSchema } from "@rjsf/utils";
import type { JsonSchema } from "../src/components/forms/types";
import { DNS1123 } from "../src/schemas/kinds";
import {
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectOpen,
  expectTabOrder,
} from "./checks";

interface Space {
  name?: string;
  description?: string;
  retentionDays?: number;
  public?: boolean;
}

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name", pattern: DNS1123 },
    description: { type: "string", title: "Description" },
    retentionDays: { type: "number", title: "Retention (days)", minimum: 1 },
    public: { type: "boolean", title: "Public" },
  },
};

function show(props: Partial<React.ComponentProps<typeof SchemaForm<Space>>> = {}) {
  const onSubmit = vi.fn();
  const onChange = vi.fn();
  const rendered = render(
    <I18nextProvider i18n={i18n}>
      <SchemaForm<Space> schema={SCHEMA} onSubmit={onSubmit} onChange={onChange} {...props} />
    </I18nextProvider>,
  );
  return { ...rendered, onSubmit, onChange };
}

/**
 * One `ErrorSchema` node, built the way `ResourceFormDialog.atPath` builds it: the `__errors`
 * array is what puts `aria-invalid` on the field and its sentence in `aria-describedby`.
 */
function errorOn(field: string, message: string): ErrorSchema {
  const root: ErrorSchema = {};
  (root as Record<string, unknown>)[field] = { __errors: [message] };
  return root;
}

const submit = () => screen.getByRole("button", { name: new RegExp(en.form.submit) });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("what a field refuses is on the field (UI-04, UI-44)", () => {
  it("a_refused_field_is_marked_invalid_and_names_its_own_message", async () => {
    show({ formData: {} });
    const name = screen.getByLabelText(/Name/);
    await userEvent.type(name, "Ovzduší");
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    // The name of the rule, not the rule: "does not match pattern" leaves a person holding a
    // regular expression (T-0960, PF-09).
    expect(name).toHaveAccessibleDescription(new RegExp(en.form.dns1123));
  });

  it("there_is_no_error_list_above_the_form_repeating_them", async () => {
    const { container } = show({ formData: {} });
    await userEvent.type(screen.getByLabelText(/Name/), "Ovzduší");
    await waitFor(() =>
      expect(screen.getByLabelText(/Name/)).toHaveAttribute("aria-invalid", "true"),
    );
    // One message, on its own field: `showErrorList={false}`.
    expect(container.querySelectorAll(`li`)).toHaveLength(1);
  });

  it("a_finding_the_page_already_has_reaches_the_field_it_belongs_to", async () => {
    show({
      formData: { name: "air" },
      extraErrors: errorOn("description", "The catalogue needs one sentence here."),
    });
    const description = screen.getByLabelText(/Description/);
    await waitFor(() => expect(description).toHaveAttribute("aria-invalid", "true"));
    expect(description).toHaveAccessibleDescription(
      new RegExp("The catalogue needs one sentence here."),
    );
  });

  // F-R2UI2-006: a draft restored or YAML applied replaces the data from outside the form.
  it("new_form_data_from_outside_replaces_what_the_form_held", () => {
    const { rerender } = show({ formData: {} });
    expect(screen.getByTestId("required-count")).toHaveTextContent("0 of 1");
    rerender(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<Space> schema={SCHEMA} onSubmit={vi.fn()} onChange={vi.fn()} formData={{ name: "air" }} />
      </I18nextProvider>,
    );
    expect(screen.getByTestId("required-count")).toHaveTextContent("1 of 1");
  });

  it("a_required_field_is_announced_as_required_not_only_starred", () => {
    show();
    expect(screen.getByLabelText(/Name/)).toBeRequired();
  });
});

describe("the submit line (UI-44, UI-15)", () => {
  it("a_closed_submit_is_reachable_and_says_why", async () => {
    show({ submitDisabledReason: "Proposing is closed while a change is under review." });
    expectDenied(submit(), "Proposing is closed while a change is under review.");
  });

  it("a_closed_submit_sends_nothing", async () => {
    const { onSubmit } = show({
      formData: { name: "air" },
      submitDisabledReason: "Your role may not propose a ContextSpace.",
    });
    await userEvent.click(submit());
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("an_open_submit_sends_what_was_typed_once", async () => {
    const { onSubmit } = show({ formData: { name: "air" } });
    expectOpen(submit());
    await userEvent.click(submit());
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ name: "air" }));
  });

  it("a_submit_in_flight_says_it_is_busy", () => {
    show({ formData: { name: "air" }, submitting: true });
    expect(submit()).toHaveAttribute("aria-busy", "true");
  });

  it("a_form_with_required_fields_says_how_many_are_filled", async () => {
    show({ formData: {} });
    expect(screen.getByTestId("required-count")).toHaveTextContent("0");
    await userEvent.type(screen.getByLabelText(/Name/), "air");
    await waitFor(() => expect(screen.getByTestId("required-count")).toHaveTextContent("1"));
  });

  it("an_empty_required_field_is_refused_and_nothing_is_sent", async () => {
    const { onSubmit } = show({ formData: {} });
    await userEvent.click(submit());
    await waitFor(() => expect(screen.getByLabelText(/Name/)).toHaveAttribute("aria-invalid", "true"));
    expect(onSubmit).not.toHaveBeenCalled();
  });
});

describe("a choice and a checkbox say it once (T-2754)", () => {
  it("a_choice_written_as_words_opens_on_no_value_unless_it_has_a_default", async () => {
    const onChange = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<{ target?: string; mode?: string }>
          schema={{
            type: "object",
            properties: {
              target: { type: "string", title: "Target", oneOf: [{ const: "a", title: "First" }, { const: "b", title: "Second" }] },
              mode: { type: "string", title: "Mode", default: "b", oneOf: [{ const: "a", title: "First" }, { const: "b", title: "Second" }] },
            },
          }}
          formData={{}}
          onSubmit={vi.fn()}
          onChange={onChange}
        />
      </I18nextProvider>,
    );
    expect(within(screen.getByLabelText(/^Target/)).getByRole("option", { selected: true })).toHaveTextContent(
      en.form.choose,
    );
    expect(within(screen.getByLabelText(/^Mode/)).getByRole("option", { selected: true })).toHaveTextContent("Second");
    await userEvent.type(screen.getByLabelText(/^Mode/), "{Tab}");
    for (const [data] of onChange.mock.calls as [{ target?: string }][]) {
      expect(data?.target).toBeUndefined();
    }
  });

  it("a_checkbox_carries_its_help_once_under_its_label", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<{ feedback?: boolean }>
          schema={{ type: "object", properties: { feedback: { type: "boolean", title: "Allow feedback", description: "For engineers." } } }}
          uiSchema={{ feedback: { "ui:help": "Loops back on purpose." } }}
          formData={{}}
          onSubmit={vi.fn()}
        />
      </I18nextProvider>,
    );
    expect(screen.getAllByText("Loops back on purpose.")).toHaveLength(1);
    expect(screen.queryByText("For engineers.")).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Allow feedback" })).toHaveAccessibleDescription(/Loops back on purpose\./);
  });
});

describe("what the person typed is not lost", () => {
  it("a_refused_submit_leaves_every_value_where_it_was", async () => {
    const { onSubmit } = show({ formData: {} });
    await userEvent.type(screen.getByLabelText(/Description/), "Air quality of the city");
    await userEvent.click(submit());
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByLabelText(/Description/)).toHaveValue("Air quality of the city");
  });
});

describe("the form meets the UI contract", () => {
  it("has_no_axe_violation_empty_filled_and_refused", async () => {
    const empty = show({ formData: {} });
    await expectNoViolations(empty.container);
    empty.unmount();

    const filled = show({ formData: { name: "air", description: "One sentence." } });
    await expectNoViolations(filled.container);
    filled.unmount();

    const refused = show({ formData: {}, submitDisabledReason: "Proposing is closed." });
    await userEvent.type(screen.getByLabelText(/Name/), "Ovzduší");
    await waitFor(() =>
      expect(screen.getByLabelText(/Name/)).toHaveAttribute("aria-invalid", "true"),
    );
    await expectNoViolations(refused.container);
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    const { container } = show({ formData: { name: "air" } });
    await expectTabOrder(user, container);
  });

  it.each(SUPPORTED_LOCALES)("says_its_own_words_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container, unmount } = show({ formData: {} });
    await userEvent.type(screen.getByLabelText(/Name/), "Ovzduší");
    await waitFor(() =>
      expect(screen.getByLabelText(/Name/)).toHaveAttribute("aria-invalid", "true"),
    );
    // The field titles come from the schema the caller passed, not from a bundle; every word
    // the form itself contributes — the submit, the count, the refusal — is translated.
    expectNoRawKeys(container);
    expect(within(container).getByRole("button", { name: new RegExp(i18n.t("form.submit")) })).toBeTruthy();
    unmount();
  });
});
