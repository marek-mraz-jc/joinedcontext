/**
 * T-5014: a picker inside a `Field` keeps the Field's hint and error ids on its input, beside its
 * own note. TypePicker and ResourceNamePicker dropped the Field's ids whenever a caller also gave
 * `describedBy`; DataModelPicker and UnitPicker never took them, so a screen reader read none of
 * the hint a person needs to choose (UI-04).
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { Field, fieldIds } from "../src/components/ui";
import { DataModelPicker } from "../src/components/pickers/DataModelPicker";
import { ResourceNamePicker } from "../src/components/pickers/ResourceNamePicker";
import { TypePicker } from "../src/components/pickers/TypePicker";
import { UnitPicker } from "../src/components/pickers/UnitPicker";

function inField(picker: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <p id="own-note">Chosen from the list.</p>
        <Field id="pick" label="Pick" description="What it is for." help="For example: air">
          {picker}
        </Field>
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

/** Every id the input's `aria-describedby` names. */
function describedIds(): string[] {
  return (screen.getByRole("combobox").getAttribute("aria-describedby") ?? "").split(" ").filter(Boolean);
}

const ids = fieldIds("pick");
const noop = () => {};

describe("a picker in a Field names the Field's hint", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] })),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const cases: [string, ReactNode][] = [
    ["TypePicker", <TypePicker id="pick" label="Pick" project="helsinki" value={[]} onChange={noop} describedBy="own-note" />],
    [
      "ResourceNamePicker",
      <ResourceNamePicker id="pick" label="Pick" from={{ project: "helsinki", plural: "spaces" }} value="" onChange={noop} describedBy="own-note" />,
    ],
    ["DataModelPicker", <DataModelPicker id="pick" label="Pick" value={[]} onChange={noop} describedBy="own-note" />],
    ["UnitPicker", <UnitPicker id="pick" label="Pick" value="" onChange={noop} describedBy="own-note" />],
  ];

  it.each(cases)("%s_keeps_its_own_note_and_the_fields_hint_and_description", (_name, picker) => {
    inField(picker);
    expect(describedIds()).toEqual(expect.arrayContaining(["own-note", ids.description, ids.help]));
    for (const id of describedIds()) expect(document.getElementById(id)).not.toBeNull();
  });
});
