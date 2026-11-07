/**
 * T-2757 (UI-16): a field shows its errors once a person reached it, never before.
 *
 * rjsf validates the whole form on every change, so typing the first letter of a sync source's
 * name turned its clone URL red before anybody got there. A field shows its errors once it was
 * changed or left, and every field does once the form was checked or a submit was refused.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ErrorSchema } from "@rjsf/utils";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { hasAnyError } from "../src/components/forms/touched";
import type { JsonSchema } from "../src/components/forms/types";
import { SyncSourcesPage } from "../src/pages/sync/SyncSourcesPage";
import { json, list, renderPage } from "./page_contract";

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name", "url"],
  properties: {
    name: { type: "string", title: "Name", pattern: "^[a-z]+$" },
    url: { type: "string", title: "Address", pattern: "^https://.+" },
  },
};

function renderForm(extraErrors?: ErrorSchema) {
  const onSubmit = vi.fn();
  const view = render(
    <I18nextProvider i18n={i18n}>
      <SchemaForm<{ name?: string; url?: string }>
        schema={SCHEMA}
        formData={{}}
        submitLabel="Propose change"
        extraErrors={extraErrors}
        onSubmit={onSubmit}
      />
    </I18nextProvider>,
  );
  return { onSubmit, view };
}

const field = (name: RegExp) => screen.getByRole("textbox", { name });
const errorOf = (id: string) => document.getElementById(`${id}__error`);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("SchemaForm errors wait for the field", () => {
  it("typing one field leaves the untouched one quiet", async () => {
    const user = userEvent.setup();
    renderForm();
    await user.type(field(/^Name/), "a");
    expect(field(/^Address/)).not.toHaveAttribute("aria-invalid", "true");
    expect(errorOf("root_url")).toBeNull();
  });

  it("a field shows its own error while it is typed in", async () => {
    const user = userEvent.setup();
    renderForm();
    await user.type(field(/^Name/), "A");
    expect(field(/^Name/)).toHaveAttribute("aria-invalid", "true");
    expect(errorOf("root_name")).not.toBeNull();
  });

  it("leaving an empty required field shows its error", async () => {
    const user = userEvent.setup();
    renderForm();
    await user.type(field(/^Name/), "a");
    await user.click(field(/^Address/));
    await user.tab();
    await waitFor(() => {
      expect(field(/^Address/)).toHaveAttribute("aria-invalid", "true");
    });
  });

  it("a refused submit shows every field's error", async () => {
    const user = userEvent.setup();
    const { onSubmit } = renderForm();
    await user.type(field(/^Name/), "a");
    await user.click(screen.getByRole("button", { name: "Propose change" }));
    await waitFor(() => {
      expect(field(/^Address/)).toHaveAttribute("aria-invalid", "true");
    });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("a check's findings show on fields nobody touched", () => {
    // Built the way ResourceFormDialog's check builds it.
    const found: ErrorSchema = {};
    (found as Record<string, unknown>).url = { __errors: ["The address does not answer."] };
    renderForm(found);
    expect(field(/^Address/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("The address does not answer.")).toBeInTheDocument();
  });
});

describe("hasAnyError", () => {
  it("finds an error at any depth and nothing in an empty schema", () => {
    expect(hasAnyError(undefined)).toBe(false);
    expect(hasAnyError({} as ErrorSchema)).toBe(false);
    const empty: ErrorSchema = {};
    (empty as Record<string, unknown>).git = { url: { __errors: [] } };
    expect(hasAnyError(empty)).toBe(false);
    const deep: ErrorSchema = {};
    (deep as Record<string, unknown>).git = { url: { __errors: ["Required"] } };
    expect(hasAnyError(deep)).toBe(true);
  });
});

describe("the sync source form", () => {
  it("typing the name leaves the clone address quiet", async () => {
    renderPage(<SyncSourcesPage project="helsinki" />, {
      path: "/projects/helsinki/syncsources",
      answer: (url, request) =>
        url.pathname.endsWith("/syncsources") && request.method === "GET"
          ? json(list([]))
          : undefined,
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: i18n.t("syncSources.add") }));
    const name = await screen.findByRole("textbox", { name: /^Name/ });
    await user.type(name, "upstream");
    const dialog = screen.getByRole("dialog");
    expect(Array.from(dialog.querySelectorAll("[aria-invalid=true]"), (node) => node.id)).toEqual([]);
  });
});
