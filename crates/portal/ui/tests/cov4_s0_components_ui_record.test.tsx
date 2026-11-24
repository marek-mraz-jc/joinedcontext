// T-7706: RecordView when the browser has no clipboard and when a record holds no field, and
// hostToShow for a safe link whose URL has no host (mailto:).
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RecordView } from "../src/components/ui";
import { recordJson } from "../src/components/ui/recordView";
import { hostToShow } from "../src/components/ui/safeHref";

const original = Object.getOwnPropertyDescriptor(window.navigator, "clipboard");

afterEach(() => {
  if (original) Object.defineProperty(window.navigator, "clipboard", original);
  else Reflect.deleteProperty(window.navigator, "clipboard");
});

const show = (value: unknown) =>
  render(
    <I18nextProvider i18n={i18n}>
      <RecordView value={value} label="Record" />
    </I18nextProvider>,
  );

describe("RecordView edges", () => {
  it("says the copy failed when the browser offers no clipboard", async () => {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, get: () => undefined });
    show({ id: "urn:ngsi-ld:Park:1" });
    await userEvent.click(screen.getByRole("button", { name: en.recordView.copy }));
    expect(screen.getByRole("alert")).toHaveTextContent(en.recordView.copyFailed);
  });

  it("names an empty record inside a list instead of drawing an empty grid", () => {
    show([{}]);
    expect(screen.getByText(en.recordView.noFields)).toBeInTheDocument();
  });

  it("names an empty list", () => {
    show([]);
    expect(screen.getByText(en.recordView.noItems)).toBeInTheDocument();
  });
});

describe("recordJson", () => {
  it("keeps a string and prints undefined as text", () => {
    expect(recordJson("raw")).toBe("raw");
    expect(recordJson(undefined)).toBe("undefined");
    expect(recordJson({ a: 1 })).toBe('{\n  "a": 1\n}');
  });
});

describe("hostToShow", () => {
  it("prints no host for a mailto link, which has none", () => {
    expect(hostToShow("write us", "mailto:office@example.org")).toBeUndefined();
  });

  it("prints the host for a label that only looks like it", () => {
    expect(hostToShow("hel.fi", "https://el.fi/login")).toBe("el.fi");
  });
});
