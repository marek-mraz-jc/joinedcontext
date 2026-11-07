/**
 * T-1817: the rjsf theme — the templates and widgets every schema-driven form of the Portal is
 * built from — against the UI contract (UI-01, UI-15, UI-16, UI-44).
 *
 * A form generated from a JSON Schema and a form written by hand are the same form, so what is
 * measured here is that each widget is the shared control, that the field's help, its example and
 * its error reach the control that carries them, and that nothing in the theme takes the focus or
 * closes a button without saying why.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { ListLegend } from "../src/components/forms/theme";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { expectDenied, expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";

/** One field of every kind the theme renders, in one form. */
const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name", description: "The schema's own words" },
    period: { type: "number", title: "Period" },
    notes: { type: "string", title: "Notes" },
    audience: { type: "string", title: "Audience", enum: ["public", "organization", "partner"] },
    published: { type: "boolean", title: "Published" },
    revision: { type: "string", title: "Revision" },
    keywords: { type: "array", title: "Keywords", items: { type: "string", title: "Keyword" } },
  },
};

const UI: UiSchema = {
  name: { "ui:help": "What people will look for it by" },
  notes: { "ui:widget": "textarea" },
  revision: { "ui:widget": "hidden" },
  period: { "ui:placeholder": "30" },
  keywords: { "ui:help": "Words a search finds it by" },
};

interface Data {
  name?: string;
  period?: number;
  notes?: string;
  audience?: string;
  published?: boolean;
  revision?: string;
  keywords?: string[];
}

function form(props: Partial<React.ComponentProps<typeof SchemaForm<Data>>> = {}) {
  const onSubmit = vi.fn();
  const view = render(
    <I18nextProvider i18n={i18n}>
      <SchemaForm<Data> schema={SCHEMA} uiSchema={UI} onSubmit={onSubmit} {...props} />
    </I18nextProvider>,
  );
  return { ...view, onSubmit, user: userEvent.setup() };
}

describe("the form theme against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("renders every widget as the shared control", () => {
    const { container } = form();

    const name = screen.getByLabelText(/Name/) as HTMLInputElement;
    const notes = screen.getByLabelText("Notes") as HTMLTextAreaElement;
    const audience = screen.getByLabelText("Audience") as HTMLSelectElement;
    const published = screen.getByLabelText("Published") as HTMLInputElement;

    expect(notes.tagName).toBe("TEXTAREA");
    expect(audience.tagName).toBe("SELECT");
    expect(published.type).toBe("checkbox");
    for (const control of [name, notes, audience, published]) {
      expect(control.className, control.outerHTML).toContain("focus-ring");
    }
    // A number field reads as a column of numbers, not as prose.
    expect((screen.getByLabelText("Period") as HTMLInputElement).className).toContain("tabular-nums");
    // Every input a person can see is the shared control; a hidden one has no appearance at all.
    expect(
      container.querySelectorAll("input:not([type='hidden']):not([class*='focus-ring'])"),
    ).toHaveLength(0);
  });

  it("keeps a colour and a size on the scale, in the one widget that has its own (a range)", () => {
    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<{ level?: number }>
          schema={{ type: "object", properties: { level: { type: "number", title: "Level", minimum: 0, maximum: 10 } } }}
          uiSchema={{ level: { "ui:widget": "range" } }}
          onSubmit={() => {}}
        />
      </I18nextProvider>,
    );

    const range = container.querySelector("input[type=range]") as HTMLInputElement;
    // `accent-primary`, not an arbitrary `accent-[var(--portal-primary)]`: the slider follows the
    // installation's brand through the same token as every checkbox.
    expect(range.className).toContain("accent-primary");
    expect(range.className).not.toMatch(/accent-\[/);
  });

  it("says what a field is for in the words written for a person, and points the control at them", () => {
    form();

    const name = screen.getByLabelText(/Name/);
    // `ui:help` is written for the reader; the schema's description is the field's rustdoc and
    // only the fallback (T-1604).
    expect(screen.getByText("What people will look for it by")).toBeInTheDocument();
    expect(screen.queryByText("The schema's own words")).toBeNull();
    expect(name).toHaveAccessibleDescription(/What people will look for it by/);
  });

  it("takes no focus of its own, and is walked in DOM order", async () => {
    const { container, user } = form();

    expect(document.activeElement).toBe(document.body);
    await expectTabOrder(user, container);
  });

  it("keeps a hidden field out of sight and in the form", async () => {
    const { container, onSubmit, user } = form({ formData: { name: "air", revision: "v7" } });

    const hidden = container.querySelector("div[hidden]") as HTMLElement;
    expect(hidden).not.toBeNull();
    expect(within(hidden).getByDisplayValue("v7")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("form.submit") }));
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ revision: "v7" }));
  });

  it("names every action of an array, and adds and removes an item", async () => {
    const { user } = form({ formData: { name: "air", keywords: ["pm10"] } });

    const add = screen.getByRole("button", { name: /Add/i });
    await user.click(add);
    expect(screen.getAllByLabelText(/Keyword/)).toHaveLength(2);
    for (const control of screen.getAllByRole("button", { name: /Remove|Move/i })) {
      expect(control).toHaveAccessibleName();
    }
    await user.click(screen.getAllByRole("button", { name: /Remove/i })[0]);
    expect(screen.getAllByLabelText(/Keyword/)).toHaveLength(1);
  });

  // T-2835: an item of a list has no words of its own; it is read with the list's.
  it("describes each item of a list with the list's hint, shown once", () => {
    form({ formData: { name: "air", keywords: ["pm10", "no2"] } });
    const items = screen.getAllByLabelText(/Keyword/);
    expect(items).toHaveLength(2);
    for (const item of items) {
      expect(item).toHaveAccessibleDescription("Words a search finds it by");
    }
    const shown = screen
      .getAllByText("Words a search finds it by")
      .filter((node) => !node.classList.contains("sr-only"));
    expect(shown).toHaveLength(1);
  });

  it("marks a field the schema refuses, and says it in words beside it", async () => {
    const { user } = form();

    await user.click(screen.getByRole("button", { name: i18n.t("form.submit") }));

    const name = screen.getByLabelText(/Name/);
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(name).toHaveAccessibleDescription(new RegExp(i18n.t("form.required")));
  });

  it("keeps a submit that is closed reachable, with its reason (UI-44)", async () => {
    form({ submitDisabledReason: "Your role does not permit a change here" });

    const submit = screen.getByRole("button", { name: i18n.t("form.submit") });
    expectDenied(submit, "Your role does not permit a change here");
    // And the reason is beside it for everyone who is not listening to a screen reader.
    expect(screen.getByRole("status")).toHaveTextContent("Your role does not permit a change here");
  });

  it("says a submit is in flight and cannot be pressed twice", async () => {
    const { onSubmit, user } = form({ formData: { name: "air" }, submitting: true });

    const submit = screen.getByRole("button", { name: i18n.t("form.submit") });
    expect(submit).toHaveAttribute("aria-busy", "true");
    await user.click(submit);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("has no axe violation, with the form filled and with it refused", async () => {
    const { container, user } = form({ formData: { name: "air", keywords: ["pm10"] } });
    await expectNoViolations(container);

    await user.clear(screen.getByLabelText(/Name/));
    await user.click(screen.getByRole("button", { name: i18n.t("form.submit") }));
    // Nothing excluded: `Field`'s error used to be a `<ul role="alert">`, which no list may
    // carry, and T-2319 split the live region from the list it holds.
    await expectNoViolations(container);
  });

  it("names the chooser of an anyOf field after its field, and it passes axe (T-4380)", async () => {
    const schema: JsonSchema = {
      type: "object",
      properties: {
        title: {
          title: "Header title",
          anyOf: [
            { type: "string", title: "One text" },
            { type: "object", title: "One per language", additionalProperties: { type: "string" } },
          ],
        },
      },
    };
    const { container, user } = form({ schema, uiSchema: {} });

    const chooser = screen.getByRole("combobox", { name: "Header title: given as" });
    expect(within(chooser).getByRole("option", { name: "One text" })).toBeInTheDocument();
    await expectNoViolations(container);
    await user.selectOptions(chooser, "One per language");
    await expectNoViolations(container);
  });

  it.each(SUPPORTED_LOCALES)("writes the theme's own words in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = form({ formData: { name: "air", keywords: ["pm10"] } });

    expectNoRawKeys(container);
  });
});

describe("a list's legend", () => {
  it("names the fieldset and marks a required list for sight only", () => {
    render(
      <fieldset>
        <ListLegend heading="Keywords" required />
      </fieldset>,
    );

    expect(screen.getByRole("group", { name: "Keywords" })).toBeInTheDocument();
  });
});
