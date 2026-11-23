/**
 * T-7585: the secret reference widget names the secret a stored `${DS_...}` value points at, adopts
 * a reference that arrives after the first render, and writes nothing once the name is emptied.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SecretRefContext, SecretRefWidget } from "../src/components/forms/widgets/SecretRef";
import type { SecretRefValue } from "../src/components/forms/widgets/SecretRef";

const SECRETS: SecretRefValue[] = [
  { name: "mqtt-mesto", key: "password", envVar: "DS_MQTT_MESTO_PASSWORD" },
  { name: "other", envVar: "DS_OTHER_TOKEN" },
];

function propsOf(value: unknown, onChange = vi.fn()): WidgetProps {
  return {
    id: "root_password",
    name: "password",
    value,
    onChange,
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    options: {},
    schema: { type: "string" },
    label: "Password",
    registry: {},
  } as unknown as WidgetProps;
}

const view = (props: WidgetProps) => (
  <I18nextProvider i18n={i18n}>
    <SecretRefContext.Provider value={{ secrets: SECRETS }}>
      <SecretRefWidget {...props} />
    </SecretRefContext.Provider>
  </I18nextProvider>
);

const name = () => screen.getByLabelText(i18n.t("datasources.secretRef.name")) as HTMLInputElement;
const key = () => screen.getByLabelText(i18n.t("datasources.secretRef.key")) as HTMLInputElement;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the secret reference of a stored value", () => {
  it("shows the secret and key a stored interpolation names", () => {
    render(view(propsOf("${DS_MQTT_MESTO_PASSWORD}")));
    expect(name()).toHaveValue("mqtt-mesto");
    expect(key()).toHaveValue("password");
  });

  it("adopts the reference once the manifest arrives, keeping the default key when the secret has none", () => {
    const { rerender } = render(view(propsOf(undefined)));
    expect(name()).toHaveValue("");
    rerender(view(propsOf("${DS_MQTT_MESTO_PASSWORD}")));
    expect(name()).toHaveValue("mqtt-mesto");
    expect(key()).toHaveValue("password");
    rerender(view(propsOf("${DS_OTHER_TOKEN}")));
    expect(name()).toHaveValue("other");
    expect(key()).toHaveValue("password");
  });

  it("writes nothing once the secret name is emptied", async () => {
    const onChange = vi.fn();
    render(view(propsOf("${DS_MQTT_MESTO_PASSWORD}", onChange)));
    await userEvent.clear(name());
    expect(onChange).toHaveBeenLastCalledWith(undefined);
  });
});
