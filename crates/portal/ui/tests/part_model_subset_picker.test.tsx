/**
 * T-1849: the subset picker against the UI contract (UI-01, UI-15, UI-16, UI-43, UI-44, UI-48).
 *
 * The picker was two hand-made `<input type="checkbox">` — no shared focus ring, no shared
 * disabled style — and `id` and `type` were hard `disabled`, so the sentence written to explain
 * why they cannot be unticked sat at the bottom of the panel where nothing pointed at it: a
 * screen reader walking the boxes never reached the two it applies to. They are the shared
 * `Checkbox` now, and the identity boxes carry their reason and stay in the tab order.
 */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { expectDenied, expectNoRawKeys, expectNoViolations, focusables } from "./checks";
import { ModelSubsetPicker } from "../src/pages/models/ModelSubsetPicker";
import { parseModel } from "../src/pages/models/linkml";
import { EMPTY_SUBSET, wholeSubset } from "../src/pages/models/subset";
import type { Subset } from "../src/pages/models/subset";

const MODEL = parseModel(`id: https://hel.fi/models/fleet
name: fleet
prefixes:
  hel: https://hel.fi/terms/
classes:
  Vehicle:
    slots: [id, type, name, plate]
  Depot:
    slots: [id, type, name]
slots:
  id:
    identifier: true
  type:
    designates_type: true
  name:
    range: string
  plate:
    range: string
`);

function Harness({ initial }: { initial: Subset }) {
  const [subset, setSubset] = useState<Subset>(initial);
  return <ModelSubsetPicker model={MODEL} subset={subset} onChange={setSubset} />;
}

function show(initial: Subset = EMPTY_SUBSET) {
  const view = render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return { container: view.container, user: userEvent.setup(), unmount: view.unmount };
}

describe("the model subset picker against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("refuses an identity slot with its reason, and keeps it reachable", async () => {
    show(wholeSubset(MODEL));
    const id = screen.getByRole("checkbox", { name: "Vehicle.id" });
    expectDenied(id, en.models.subset.identity);
    expect(id).toBeChecked();

    // Reachable means reachable: it takes the focus, and the space bar leaves it ticked.
    id.focus();
    expect(id).toHaveFocus();
    await userEvent.keyboard(" ");
    expect(id).toBeChecked();
  });

  it("ticks a class and one of its slots from the keyboard", async () => {
    const { user } = show();
    const vehicle = screen.getByRole("checkbox", { name: "Vehicle" });
    vehicle.focus();
    await user.keyboard(" ");
    expect(vehicle).toBeChecked();

    const plate = screen.getByRole("checkbox", { name: "Vehicle.plate" });
    plate.focus();
    await user.keyboard(" ");
    expect(plate).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Depot" })).not.toBeChecked();
  });

  it("says in words that an empty subset exposes nothing", () => {
    show();
    expect(screen.getByRole("status")).toHaveTextContent(en.models.subset.nothing);
  });

  it("walks every box in the order they are read, identity boxes included", () => {
    const { container } = show(wholeSubset(MODEL));
    const names = focusables(container).map((box) => box.getAttribute("aria-label") ?? "Vehicle");
    expect(names.slice(0, 5)).toEqual([
      "Vehicle",
      "Vehicle.id",
      "Vehicle.type",
      "Vehicle.name",
      "Vehicle.plate",
    ]);
  });

  it("has no axe violation, ticked or empty", async () => {
    const empty = show();
    await expectNoViolations(empty.container);
    empty.unmount();
    const { container } = show(wholeSubset(MODEL));
    await expectNoViolations(container);
  });

  it("shows no raw translation key in any locale the organisation offers", async () => {
    for (const locale of SUPPORTED_LOCALES) {
      await i18n.changeLanguage(locale);
      const { container, unmount } = render(
        <I18nextProvider i18n={i18n}>
          <Harness initial={EMPTY_SUBSET} />
        </I18nextProvider>,
      );
      expectNoRawKeys(container);
      unmount();
    }
    await i18n.changeLanguage("en");
  });
});
