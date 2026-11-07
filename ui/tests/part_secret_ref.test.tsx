/**
 * T-1820: the two halves of a secret reference, against the UI contract (UI-04, UI-15, UI-16).
 *
 * The widget's reason to exist is that a credential never reaches the form: what it writes is
 * `${DS_<NAME>_<KEY>}` and what the page collects is the reference. The contract adds that both
 * halves are the shared `Input` (one border, one ring, one disabled state), that both are named
 * for a screen reader even though the labels are visually hidden, and that both point at the
 * field's hint and the server's refusal — nothing above a control reaches it on its own (T-2314).
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { SecretRefContext } from "../src/components/forms/widgets/SecretRef";
import type { SecretRefValue } from "../src/components/forms/widgets/SecretRef";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { expectNoRawKeys, expectNoViolations, focusables } from "./checks";

const SCHEMA: JsonSchema = {
  type: "object",
  properties: { password: { type: "string", title: "Password", description: "From the broker" } },
};

const UI: UiSchema = { password: { "ui:widget": "secretRef" } };

function show(
  context: {
    knownSecretNames?: string[];
    onSecretRef?: (envVar: string, ref: SecretRefValue) => void;
  } = {},
) {
  return render(
    <I18nextProvider i18n={i18n}>
      <SecretRefContext.Provider value={context}>
        <SchemaForm schema={SCHEMA} uiSchema={UI} onSubmit={() => {}} />
      </SecretRefContext.Provider>
    </I18nextProvider>,
  );
}

const name = () => screen.getByLabelText(i18n.t("datasources.secretRef.name")) as HTMLInputElement;
const key = () => screen.getByLabelText(i18n.t("datasources.secretRef.key")) as HTMLInputElement;

describe("the secret reference widget against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names both halves for a screen reader, and both are the shared control", () => {
    show();

    expect(name().tagName).toBe("INPUT");
    expect(name().className).toContain("focus-ring");
    expect(key().className).toContain("focus-ring");
    // The labels are visually hidden — the field above says what the field is — but they are
    // there, so neither half is an unnamed box in a screen reader's list of controls.
    expect(name()).toHaveAccessibleName(i18n.t("datasources.secretRef.name"));
    expect(key()).toHaveAccessibleName(i18n.t("datasources.secretRef.key"));
  });

  it("points both halves at the field's own hint (T-2314, UI-04)", () => {
    show();
    // One field, two controls: the description belongs to both, or the second one is unexplained.
    expect(name()).toHaveAccessibleDescription(/From the broker/);
    expect(key()).toHaveAccessibleDescription(/From the broker/);
  });

  it("is filled by keyboard alone, in the order it is read", async () => {
    const user = userEvent.setup();
    const announced = vi.fn();
    const { container } = show({ onSecretRef: announced });

    const controls = focusables(container).filter((element) => element.tagName === "INPUT");
    expect(controls.slice(0, 2)).toEqual([name(), key()]);

    name().focus();
    await user.keyboard("mqtt-credentials");
    await user.tab();
    expect(key()).toHaveFocus();
    await user.keyboard("password");

    // What leaves the widget is the reference, never the value typed into a credential field.
    expect(announced).toHaveBeenCalled();
    const [envVar, ref] = announced.mock.calls[announced.mock.calls.length - 1] as [string, SecretRefValue];
    expect(envVar).toBe("DS_MQTT_CREDENTIALS_PASSWORD");
    expect(ref).toMatchObject({ name: "mqtt-credentials", key: "password" });
  });

  it("shows the reference it wrote, and no credential anywhere on the page", async () => {
    const user = userEvent.setup();
    const { container } = show();

    // Only the secret: the key defaults to the field's own name, which is what a `password`
    // field needs and all the form ever holds.
    await user.type(name(), "mqtt-credentials");

    const written = await screen.findByTestId("root_password__interpolation");
    expect(written.textContent).toBe("${DS_MQTT_CREDENTIALS_PASSWORD}");
    // Nothing on the page holds a value: both halves name a secret, they do not carry one.
    expect(container.textContent).not.toMatch(/hunter2|s3cret/);
  });

  it("has no axe violation, empty and filled", async () => {
    const user = userEvent.setup();
    const { container } = show({ knownSecretNames: ["mqtt-credentials", "hsl-token"] });

    await expectNoViolations(container);
    await user.type(name(), "mqtt-credentials");
    await user.type(key(), "password");
    await expectNoViolations(container);
  });

  it.each(SUPPORTED_LOCALES)("names both halves in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = show();

    expect(screen.getByLabelText(i18n.t("datasources.secretRef.name"))).toBeInTheDocument();
    expect(screen.getByLabelText(i18n.t("datasources.secretRef.key"))).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});
