/**
 * The widget behind a field marked `secret: true` (MF-35, PL-16, PL-50, T-0963).
 *
 * What it writes into the form is a reference — `${DS_<NAME>_<KEY>}` — and never the credential,
 * and what the page collects into `spec.secrets` is what it announces. Both halves are asserted
 * here, and so is the seam the announcement travels through: rjsf passes no widget props of its
 * own down a nested form, so the widget reads its page through a React context. The three
 * sources it reads in order — the field's own options, the form context, then the React context
 * — are the reason a claim that the context is unused is wrong.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { SecretRefContext } from "../src/components/forms/widgets/SecretRef";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";

const SCHEMA: JsonSchema = {
  type: "object",
  properties: { password: { type: "string", title: "Password" } },
};

const UI: UiSchema = { password: { "ui:widget": "secretRef" } };

function renderWidget(context: {
  knownSecretNames?: string[];
  onSecretRef?: (envVar: string, ref: { name?: string; key?: string }) => void;
}) {
  render(
    <I18nextProvider i18n={i18n}>
      <SecretRefContext.Provider value={context}>
        <SchemaForm schema={SCHEMA} uiSchema={UI} onSubmit={() => {}} />
      </SecretRefContext.Provider>
    </I18nextProvider>,
  );
}

describe("the secret reference widget", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("offers the secrets the page knows about, through the context that carries them", async () => {
    renderWidget({ knownSecretNames: ["mqtt-credentials", "hsl-token"] });

    const options = [...document.querySelectorAll("datalist option")].map((option) =>
      option.getAttribute("value"),
    );
    expect(options).toContain("mqtt-credentials");
    expect(options).toContain("hsl-token");
  });

  /// The page collects `spec.secrets` from what the widget announces, and the announcement
  /// reaches it through the same context (PL-16).
  it("announces the reference a person typed, never the value", async () => {
    const announced = vi.fn();
    renderWidget({ onSecretRef: announced });

    const [name] = screen.getAllByRole("textbox");
    await userEvent.type(name, "mqtt-credentials");

    expect(announced).toHaveBeenCalled();
    const [envVar, reference] = announced.mock.calls[announced.mock.calls.length - 1];
    expect(envVar).toMatch(/^DS_MQTT_CREDENTIALS_/);
    expect(reference).toMatchObject({ name: "mqtt-credentials" });
    // Nothing that looks like a credential travels: a reference is a name and a key.
    expect(JSON.stringify(reference)).not.toContain("hunter2");
  });

  it("announces the reference once per keystroke, not twice", async () => {
    const announced = vi.fn();
    renderWidget({ onSecretRef: announced });

    const [name] = screen.getAllByRole("textbox");
    await userEvent.type(name, "abc");

    expect(announced).toHaveBeenCalledTimes(3);
  });

  it("writes an interpolation into the form and never a secret", async () => {
    const submitted: unknown[] = [];
    render(
      <I18nextProvider i18n={i18n}>
        <SecretRefContext.Provider value={{}}>
          <SchemaForm
            schema={SCHEMA}
            uiSchema={UI}
            submitLabel="Save"
            onSubmit={(data) => submitted.push(data)}
          />
        </SecretRefContext.Provider>
      </I18nextProvider>,
    );

    // The key defaults to the field's own name, so a `password` field needs only the secret.
    const [name] = screen.getAllByRole("textbox");
    await userEvent.type(name, "mqtt-credentials");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(submitted).toHaveLength(1);
    const value = (submitted[0] as { password?: string }).password ?? "";
    expect(value).toMatch(/^\$\{DS_MQTT_CREDENTIALS_PASSWORD\}$/);
  });
});
