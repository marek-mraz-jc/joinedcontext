/**
 * T-1824: the one chip for a lifecycle, against the UI contract (UI-16, UI-30, UI-48).
 *
 * A lane and a phase are the two things every list and every dialog shows about a resource, and
 * the rule about them is UI-30: the colour is never the only carrier of meaning. So the chip has
 * to carry the word in the reader's language, keep the dot decorative, and say a value it does
 * not know rather than swallow it.
 */
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { LifecycleBadge } from "../src/components/status/LifecycleBadge";
import type { LifecycleKind } from "../src/components/status/LifecycleBadge";
import { expectNoRawKeys, expectNoViolations } from "./checks";

function show(kind: LifecycleKind, value?: string | null) {
  return render(
    <I18nextProvider i18n={i18n}>
      <p>
        <LifecycleBadge kind={kind} value={value} />
      </p>
    </I18nextProvider>,
  );
}

describe("the lifecycle badge against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it.each([
    ["lane", "green", "lane.green"],
    ["lane", "yellow", "lane.yellow"],
    ["lane", "red", "lane.red"],
    ["phase", "PendingApproval", "phase.pendingApproval"],
    ["phase", "Live", "phase.live"],
    ["phase", "OutOfSync", "phase.outOfSync"],
    ["appLifecycle", "published", "appLifecycle.published"],
  ])("says %s %s in words, not only in colour", (kind, value, key) => {
    const { container } = show(kind as LifecycleKind, value);

    expect(container).toHaveTextContent(i18n.t(key));
    // The dot is decoration; the word is the meaning.
    const dot = container.querySelector("[aria-hidden=true]");
    expect(dot).not.toBeNull();
    expect(dot).toBeEmptyDOMElement();
  });

  it("carries the explanation as the chip's own tooltip", () => {
    show("phase", "Drifted");
    const chip = screen.getByTitle(i18n.t("phase.driftedHelp"));
    expect(chip).toHaveTextContent(i18n.t("phase.drifted"));
  });

  it("says a value it does not know instead of swallowing it", () => {
    const { container } = show("phase", "SomethingNewFromTheApi");
    expect(container).toHaveTextContent("SomethingNewFromTheApi");
    // An unknown value gets the quiet tone, never a green one that would claim it is fine.
    expect(container.querySelector("span[class*=surface-subtle]")).not.toBeNull();
  });

  it("is empty-handed rather than wrong when there is no value at all", () => {
    const { container } = show("lane", null);
    expect(container.textContent?.trim()).toBe("");
    expect(screen.queryByTitle(/./)).toBeNull();
  });

  it.each([
    ["lane", "green"],
    ["phase", "PendingApproval"],
    ["appLifecycle", "draft"],
  ])("has no axe violation for a %s chip", async (kind, value) => {
    const { container } = show(kind as LifecycleKind, value);
    await expectNoViolations(container);
  });

  it.each(SUPPORTED_LOCALES)("writes the lane and the phase in %s", async (locale) => {
    await i18n.changeLanguage(locale);

    const lane = show("lane", "yellow");
    expect(lane.container).toHaveTextContent(i18n.t("lane.yellow"));
    expectNoRawKeys(lane.container);
    lane.unmount();

    const phase = show("phase", "Live");
    expect(phase.container).toHaveTextContent(i18n.t("phase.live"));
    expectNoRawKeys(phase.container);
  });
});
