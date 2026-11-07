/**
 * T-4212 (UI-15, ui-writing rule 1): a list's buttons speak the page's language. rjsf words them
 * through its English translator unless the form hands it one, so the organization's new-role
 * form said "Add" on a Slovak page (photo audit T-4158).
 */
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema } from "../src/components/forms/types";

const SCHEMA: JsonSchema = {
  type: "object",
  properties: { rules: { type: "array", title: "Rules", items: { type: "string", title: "Rule" } } },
};

function show() {
  render(
    <I18nextProvider i18n={i18n}>
      <SchemaForm<Record<string, unknown>> schema={SCHEMA} formData={{ rules: ["a"] }} submitLabel="Save" onSubmit={vi.fn()} />
    </I18nextProvider>,
  );
}

afterEach(async () => {
  await i18n.changeLanguage("en");
});

describe("a list's buttons", () => {
  it("say Add and Remove in Slovak on a Slovak page", async () => {
    await i18n.changeLanguage("sk");
    show();
    expect(await screen.findByRole("button", { name: "Pridať" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Odstrániť" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
  });

  it("say Add in English on an English page", async () => {
    show();
    expect(await screen.findByRole("button", { name: "Add" })).toBeInTheDocument();
  });
});
