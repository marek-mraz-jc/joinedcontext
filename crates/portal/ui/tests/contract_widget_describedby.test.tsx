/**
 * T-2314: a Portal widget names the Field's messages, because nothing else does it for it
 * (UI-04, UI-16).
 *
 * rjsf's `SchemaField` hands `FieldTemplate` a `Fragment`, and a Fragment takes no props: the
 * `aria-*` a `Field` clones onto its single child are dropped there and never reach a control.
 * rjsf's own widgets survive that because `DefaultBaseInput` names `ariaDescribedByIds(id)`
 * itself and those ids are, character for character, the ones `fieldIds` mints. The Portal's own
 * widgets are outside both mechanisms, so each one names the ids itself or the field's hint, its
 * example and the server's refusal are read by nobody.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { ariaDescribedByIds, descriptionId, errorId, helpId } from "@rjsf/utils";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { fieldIds } from "../src/components/ui";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";

const wrap = (node: React.ReactNode) =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );

/** The field carries a description, so the Field renders a message worth naming. */
const schemaFor = (): JsonSchema => ({
  type: "object",
  properties: {
    subject: { type: "string", title: "Subject", description: "What the field is for." },
  },
});

function renderWidget(widget: string) {
  const ui: UiSchema = { subject: { "ui:widget": widget } };
  wrap(<SchemaForm schema={schemaFor()} uiSchema={ui} onSubmit={() => {}} />);
}

/** Every control the widget rendered for the field: what a person tabs to and types into. */
function controlsOf(): HTMLElement[] {
  return Array.from(document.querySelectorAll("input, select, textarea"));
}

describe("a Portal widget names the ids its Field minted", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("the_ids_a_field_mints_are_the_ids_rjsf_points_a_control_at", () => {
    // The whole rjsf path holds together only because these two agree. They were written twice,
    // in two packages, and nothing checked that they still say the same thing.
    const ids = fieldIds("root_subject");
    expect([ids.description, ids.help, ids.error].sort()).toEqual(
      [descriptionId("root_subject"), helpId("root_subject"), errorId("root_subject")].sort(),
    );
    expect(ariaDescribedByIds("root_subject").split(" ").sort()).toEqual(
      [ids.error, ids.description, ids.help].sort(),
    );
  });

  for (const widget of ["secretRef", "entityPicker", "resourcePicker", "operations"]) {
    it(`${widget}_names_the_fields_description_on_every_control_it_renders`, () => {
      renderWidget(widget);
      const controls = controlsOf();
      expect(controls.length, `${widget} renders at least one control`).toBeGreaterThan(0);
      for (const control of controls) {
        const described = (control.getAttribute("aria-describedby") ?? "").split(/\s+/);
        expect(
          described,
          `${widget}: ${control.id || control.tagName} says nothing about the field's description`,
        ).toContain(descriptionId("root_subject"));
      }
      expect(screen.getByText("What the field is for.")).toHaveAttribute(
        "id",
        descriptionId("root_subject"),
      );
    });
  }
});
