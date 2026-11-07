/**
 * T-4207 (UI-45, ui-writing rule 3): a field says each thing once.
 *
 * The photo audit (T-4158) found every schema form printing a field's error twice after Check:
 * rjsf's live validation marks the empty field, and the Check hands the same schema verdict back
 * as `extraErrors`, which rjsf adds to its own. A role picked from a list of `const` branches
 * printed "Invalid value" once per branch, about thirty times. A message differing from the
 * schema's, such as the server's, still shows beside it (T-2731).
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ErrorSchema } from "@rjsf/utils";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema } from "../src/components/forms/types";

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name", "role", "kinds"],
  properties: {
    name: { type: "string", title: "Name" },
    role: {
      type: "string",
      title: "Role",
      oneOf: ["viewer", "editor", "steward", "approver"].map((value) => ({ const: value, title: value })),
    },
    kinds: { type: "array", title: "Kinds", minItems: 1, uniqueItems: true, items: { type: "string", enum: ["App", "Endpoint"] } },
  },
};

/**
 * Mounted, edited (so rjsf's live validation has run), then handed the Check's verdict, the way
 * ResourceFormDialogBody does it.
 */
async function renderForm(formData: Record<string, unknown>, extraErrors: ErrorSchema) {
  const form = (errors?: ErrorSchema) => (
    <I18nextProvider i18n={i18n}>
      <SchemaForm<Record<string, unknown>>
        schema={SCHEMA}
        formData={formData}
        submitLabel="Propose change"
        extraErrors={errors}
        onSubmit={vi.fn()}
      />
    </I18nextProvider>
  );
  const view = render(form());
  const user = userEvent.setup();
  const name = screen.getByRole("textbox", { name: /^Name/ });
  await user.type(name, "a");
  await user.clear(name);
  if (formData.name) await user.type(name, String(formData.name));
  view.rerender(form(extraErrors));
}

/** The Check's verdict on the name field, as ResourceFormDialogBody hands it over. */
function nameErrors(...messages: string[]): ErrorSchema {
  const found: ErrorSchema = {};
  (found as Record<string, unknown>).name = { __errors: messages };
  return found;
}

const messagesOf = (id: string) =>
  [...(document.getElementById(`${id}__error`)?.querySelectorAll("li") ?? [])].map((item) => item.textContent);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("a field's errors after Check", () => {
  it("shows the required message once when Check repeats the live verdict", async () => {
    await renderForm({}, nameErrors("This field is required"));
    expect(messagesOf("root_name")).toEqual(["This field is required"]);
  });

  it("keeps a different finding beside the schema's own", async () => {
    await renderForm({}, nameErrors("This field is required", "Another project holds this name"));
    expect(messagesOf("root_name")).toEqual(["This field is required", "Another project holds this name"]);
  });

  it("gives a choice that matches no branch one message saying what to do", async () => {
    await renderForm({ name: "a", role: "nobody", kinds: ["App"] }, nameErrors("Another project holds this name"));
    expect(messagesOf("root_role")).toEqual(["Value must be one of the allowed choices"]);
  });

  it("asks for at least one item in an empty list", async () => {
    await renderForm({ name: "a", role: "viewer", kinds: [] }, nameErrors("Another project holds this name"));
    expect(screen.getByText("Choose at least one")).toBeInTheDocument();
    expect(screen.queryByText("Invalid value")).toBeNull();
  });
});
