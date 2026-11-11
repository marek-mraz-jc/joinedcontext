import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceFormNotes } from "../src/components/forms/ResourceFormNotes";

const notes = (bindsEndpoints: string[]) => (
  <I18nextProvider i18n={i18n}>
    <ResourceFormNotes conflict={null} saveRefused={null} draft={null} verdict={null} restartsStream={false} bindsEndpoints={bindsEndpoints} />
  </I18nextProvider>
);

describe("a Policy's plan names the Endpoints it binds (CHG-11, T-6740)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("lists each Endpoint whose calls the checked Policy decides", () => {
    render(notes(["air-open", "air-partners"]));
    const note = screen.getByTestId("binds-endpoints");
    expect(note).toHaveTextContent(en.form.bindsEndpoints);
    expect(within(note).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["air-open", "air-partners"]);
  });

  it("says nothing when it binds none", () => {
    render(notes([]));
    expect(screen.queryByTestId("binds-endpoints")).toBeNull();
  });
});
