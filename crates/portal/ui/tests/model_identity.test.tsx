/** T-6162: the editor's name line, a rename behind a pencil, the id and the file locked. */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ModelIdentity, setModelTitle } from "../src/pages/models/ModelIdentity";
import { edit, parseModel } from "../src/pages/models/linkml";
import { expectNoViolations } from "./checks";

function renderIdentity(props: Partial<Parameters<typeof ModelIdentity>[0]> = {}) {
  const onRename = vi.fn();
  const view = render(
    <I18nextProvider i18n={i18n}>
      <ModelIdentity name="Air quality" id="air-quality" creating={false} version="1.2.0" onRename={onRename} {...props} />
    </I18nextProvider>,
  );
  return { onRename, user: userEvent.setup(), ...view };
}

const rename = en.models.identity.rename.replace("{name}", "Air quality");

describe("ModelIdentity", () => {
  it("renames behind the pencil and leaves the id and the file as they are", async () => {
    const { onRename, user, container } = renderIdentity();
    expect(screen.getByText("air-quality")).toBeInTheDocument();
    expect(screen.getByText("air-quality.linkml.yaml")).toBeInTheDocument();
    expect(screen.getByText("v1.2.0")).toBeInTheDocument();
    // Neither the id nor the file name is a control a person can change.
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: rename }));
    const field = screen.getByRole("textbox", { name: en.models.identity.name });
    expect(field).toHaveAccessibleDescription(en.models.identity.nameHint);
    await user.clear(field);
    await user.type(field, "Air quality stations{Enter}");

    expect(onRename).toHaveBeenCalledWith("Air quality stations");
    expect(screen.getByRole("status")).toHaveTextContent(en.models.identity.renamed.replace("{id}", "air-quality"));
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    await expectNoViolations(container);
  });

  it("refuses an empty name and cancels on Escape", async () => {
    const { onRename, user } = renderIdentity();
    await user.click(screen.getByRole("button", { name: rename }));
    const field = screen.getByRole("textbox", { name: en.models.identity.name });
    await user.clear(field);
    await user.click(screen.getByRole("button", { name: en.models.identity.done }));
    expect(screen.getByText(en.models.identity.nameEmpty)).toBeInTheDocument();
    expect(onRename).not.toHaveBeenCalled();

    await user.type(field, "x{Escape}");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(onRename).not.toHaveBeenCalled();
  });

  // A new model is named in the New data model window (owner 2026-11-01, T-6344); the page renames
  // it behind the pencil like a saved one.
  it("renames a new model behind the pencil, before it has an id", async () => {
    const { onRename, user } = renderIdentity({ id: undefined, creating: true, version: undefined });
    expect(screen.getByText(en.models.identity.noId)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: rename }));
    const field = screen.getByRole("textbox", { name: en.models.identity.name });
    await user.clear(field);
    await user.type(field, "Bikes{Enter}");
    expect(onRename).toHaveBeenLastCalledWith("Bikes");
  });

  it("offers no rename to a person who may not propose", () => {
    renderIdentity({ readOnly: true });
    expect(screen.queryByRole("button", { name: rename })).not.toBeInTheDocument();
    expect(screen.getByText("Air quality")).toBeInTheDocument();
  });
});

describe("setModelTitle", () => {
  it("writes the English title and keeps the other languages", () => {
    const source = "id: https://example.org/m\nname: m\ntitle:\n  en: Old\n  sk: Starý\n";
    const renamed = edit(source, (document) => setModelTitle(document, "New"));
    expect(parseModel(renamed).title).toEqual({ en: "New", sk: "Starý" });
    expect(parseModel(renamed).name).toBe("m");
  });

  it("gives a model without a title an English one", () => {
    const renamed = edit("id: https://example.org/m\nname: m\n", (document) => setModelTitle(document, "New"));
    expect(parseModel(renamed).title).toEqual({ en: "New" });
  });
});
