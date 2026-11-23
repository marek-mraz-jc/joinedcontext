/**
 * T-7585: "Show the first field to fix" after a refused save leads to the field, opening the folds
 * it sits in, and focuses it.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema } from "../src/components/forms/types";

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: { name: { type: "string", title: "Name" } },
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the first field to fix", () => {
  it("opens the fold around it, scrolls to it and focuses it", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    const onSubmit = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <details>
          <summary>Fold</summary>
          <SchemaForm schema={SCHEMA} onSubmit={onSubmit} formData={{}} />
        </details>
      </I18nextProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: new RegExp(en.form.submit) }));
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByRole("status", { name: "" })).toHaveTextContent(en.form.fixCount.replace("{count}", "1"));
    await userEvent.click(screen.getByRole("button", { name: en.form.fixFirst }));
    const field = screen.getByLabelText(/Name/);
    expect(document.activeElement).toBe(field);
    expect(scroll).toHaveBeenCalledWith({ block: "center" });
    expect(document.querySelector("details")?.open).toBe(true);
  });
});
