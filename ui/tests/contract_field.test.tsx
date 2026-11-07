/**
 * T-1739, T-1740: the Field/Input contract (UI-01, UI-15, UI-16, UI-44).
 *
 * A Field renders a label, a description, a control, a hint and the errors. Until now it minted
 * the ids for those messages and stamped them on its own paragraphs, and left every page to
 * reconstruct `${id}__help` by hand and name it in the control's `aria-describedby`. Two of the
 * twenty message-bearing Fields did. These cases are the other eighteen.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Field.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { Field, Input, Select, fieldIds } from "../src/components/ui";
import { expectNoViolations } from "./checks";

const wrap = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("a Field wires what it renders", () => {
  it("the_help_a_field_shows_is_announced_without_the_page_repeating_the_id", () => {
    wrap(
      <Field id="slug" label="Endpoint name" help="Lower case, digits and hyphens.">
        <Input id="slug" />
      </Field>,
    );
    const control = screen.getByLabelText("Endpoint name");
    expect(control).toHaveAccessibleDescription("Lower case, digits and hyphens.");
    expect(control.getAttribute("aria-describedby")).toContain(fieldIds("slug").help);
  });

  it("a_description_is_read_before_the_control_and_the_error_after_it", () => {
    wrap(
      <Field
        id="ns"
        label="Namespace"
        description="Where the entities are written."
        help="It cannot be changed later."
        errors={["A namespace is required.", "It must be a DNS label."]}
      >
        <Input id="ns" />
      </Field>,
    );
    const control = screen.getByLabelText(/Namespace/);
    const described = control.getAttribute("aria-describedby")?.split(" ") ?? [];
    const ids = fieldIds("ns");
    expect(described).toEqual([ids.description, ids.help, ids.error]);
  });

  it("a_field_with_errors_marks_its_own_control_invalid", () => {
    // `aria-invalid` is what draws the red border — `Input`'s own `aria-[invalid=true]:border-danger`
    // — and it was the caller's job, so a Field with errors commonly showed a normal control.
    wrap(
      <Field id="port" label="Port" errors={["Not a number."]}>
        <Input id="port" />
      </Field>,
    );
    expect(screen.getByLabelText("Port")).toHaveAttribute("aria-invalid", "true");
  });

  it("every_error_is_its_own_line_not_a_comma_spliced_sentence", () => {
    wrap(
      <Field id="ns" label="Namespace" errors={["A namespace is required.", "It must be a DNS label."]}>
        <Input id="ns" />
      </Field>,
    );
    const alert = screen.getByRole("alert");
    // The live region holds the list; it is not the list itself (T-2319): `alert` allows no
    // child with a list role, so a `<ul role="alert">` loses the list semantics it was written
    // for and axe reports `aria-allowed-role` on it.
    expect(alert.tagName).toBe("DIV");
    expect(within(alert).getByRole("list").querySelectorAll("li")).toHaveLength(2);
    expect(alert.textContent).not.toContain("required., It");
  });

  it("a_required_field_says_so_through_aria_required_not_a_red_asterisk_alone", () => {
    // The asterisk is `aria-hidden` and nothing else said so: a screen reader was told nothing
    // at all, because the Field never put `aria-required` on the control and no caller did.
    wrap(
      <Field id="name" label="Name" required>
        <Input id="name" />
      </Field>,
    );
    const control = screen.getByLabelText(/Name/);
    expect(control).toHaveAttribute("aria-required", "true");
    // And the name stays the field's name — a hidden word in the label joins it.
    expect(control).toHaveAccessibleName("Name");
  });

  it("what_the_control_already_carries_is_kept_and_never_overwritten", () => {
    wrap(
      <Field id="q" label="Filter" help="An NGSI-LD query." errors={["Bad query."]}>
        <Input id="q" aria-describedby="page-hint" aria-invalid="false" />
      </Field>,
    );
    const control = screen.getByLabelText("Filter");
    const described = control.getAttribute("aria-describedby")?.split(" ") ?? [];
    expect(described[0], "the caller's own id stays first").toBe("page-hint");
    expect(described).toContain(fieldIds("q").help);
    // An explicit `aria-invalid` from the page is the page's decision, not the Field's.
    expect(control).toHaveAttribute("aria-invalid", "false");
  });

  it("a_field_with_nothing_to_say_touches_its_control_at_all", () => {
    wrap(
      <Field id="plain" label="Title">
        <Input id="plain" />
      </Field>,
    );
    const control = screen.getByLabelText("Title");
    expect(control).not.toHaveAttribute("aria-describedby");
    expect(control).not.toHaveAttribute("aria-invalid");
    expect(control).not.toHaveAttribute("aria-required");
  });

  it("it_wires_a_select_the_same_way_it_wires_an_input", () => {
    wrap(
      <Field id="rep" label="Representation" help="How the data is served.">
        <Select id="rep">
          <option value="ngsi-ld">NGSI-LD</option>
        </Select>
      </Field>,
    );
    expect(screen.getByLabelText("Representation")).toHaveAccessibleDescription(
      "How the data is served.",
    );
  });
});

/**
 * T-2319: the refused field is announced, keeps its list, and is clean under axe (UI-16, UI-44).
 *
 * `Field` marked its error list `<ul role="alert">`. `alert` allows no child with a list role, so
 * axe reported `aria-allowed-role` on every refused field in the Portal — one violation in the
 * shared component, hundreds on the pages — and a screen reader was free to drop the list on the
 * one message a person needs when the form refuses what they typed.
 */
describe("a refused field is announced without losing its list", () => {
  it("a_refused_field_has_no_axe_violation", async () => {
    const { container } = wrap(
      <Field id="port" label="Port" help="Between 1 and 65535." errors={["Not a number.", "It is required."]}>
        <Input id="port" />
      </Field>,
    );
    await expectNoViolations(container);
  });

  it("the_announced_element_is_the_one_the_control_is_described_by", () => {
    wrap(
      <Field id="ns" label="Namespace" errors={["A namespace is required."]}>
        <Input id="ns" />
      </Field>,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveAttribute("id", fieldIds("ns").error);
    expect(screen.getByLabelText("Namespace")).toHaveAccessibleDescription(
      "A namespace is required.",
    );
  });

  it("one_message_is_still_one_line_and_two_are_still_two", () => {
    const { rerender } = wrap(
      <Field id="a" label="A" errors={["One."]}>
        <Input id="a" />
      </Field>,
    );
    expect(within(screen.getByRole("alert")).getAllByRole("listitem")).toHaveLength(1);
    rerender(
      <I18nextProvider i18n={i18n}>
        <Field id="a" label="A" errors={["One.", "Two."]}>
          <Input id="a" />
        </Field>
      </I18nextProvider>,
    );
    expect(within(screen.getByRole("alert")).getAllByRole("listitem")).toHaveLength(2);
  });
});
