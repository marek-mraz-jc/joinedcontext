/**
 * T-1799: the hand-off wrapper against the UI contract (UI-15, UI-16, UI-46).
 *
 * It renders no markup of its own — it is a keyed `Fragment` — so what the contract asks of it is
 * that it stays that way: nothing added around a page, nothing announced, no focus moved, and the
 * page it wraps still axe-clean and unchanged in the four locales. What it does do is remount the
 * page when the assistant hands it something new, which is the part that can regress silently.
 */
import { render, screen } from "@testing-library/react";
import { useEffect, useState } from "react";
import type { JSX } from "react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations } from "./checks";

let searchStr = "";
let mounts = 0;
let flushes = 0;

vi.mock("@tanstack/react-router", () => ({
  useRouter: () => ({
    history: {
      flush: () => {
        flushes += 1;
      },
    },
  }),
  useRouterState: ({ select }: { select: (state: { location: { searchStr: string } }) => string }) =>
    select({ location: { searchStr } }),
}));

const { HandOff, HAND_OFF } = await import("../src/assistant/HandOff");

/** A page as every hand-off page is written: it takes what it was handed as it mounts. */
function Page(): JSX.Element {
  const [opened] = useState(() => new URLSearchParams(searchStr).get("endpoint") ?? "");
  useEffect(() => {
    mounts += 1;
  }, []);
  return (
    <main>
      <h1>{i18n.t("explore.title")}</h1>
      <p>{opened === "" ? i18n.t("explore.noEndpoint") : opened}</p>
    </main>
  );
}

function show() {
  return render(
    <I18nextProvider i18n={i18n}>
      <HandOff>
        <Page />
      </HandOff>
    </I18nextProvider>,
  );
}

describe("the hand-off wrapper against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    mounts = 0;
    flushes = 0;
    searchStr = "";
  });

  it("puts nothing of its own around the page", () => {
    searchStr = "?endpoint=public-air";
    const { container } = show();

    // One child, the page itself: no wrapper element, no landmark, no announcement.
    expect(container.children).toHaveLength(1);
    expect(container.firstElementChild?.tagName).toBe("MAIN");
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("moves no focus of its own", () => {
    searchStr = "?endpoint=public-air";
    show();
    expect(document.body).toHaveFocus();
  });

  it("has no axe violation around the page it wraps", async () => {
    searchStr = "?endpoint=public-air";
    const { container } = show();
    await expectNoViolations(container);
  });

  it("reads the address before the page does", () => {
    searchStr = "?endpoint=public-air";
    show();
    // The router moves before the address does, so the flush is what makes the page read the
    // hand-off it was actually sent (T-0770).
    expect(flushes).toBeGreaterThan(0);
    expect(screen.getByText("public-air")).toBeInTheDocument();
  });

  it("mounts the page again for a new hand-off and leaves it alone for anything else", () => {
    searchStr = "?endpoint=public-air";
    const { rerender } = show();
    expect(mounts).toBe(1);

    searchStr = "?endpoint=public-air&page=2";
    rerender(
      <I18nextProvider i18n={i18n}>
        <HandOff>
          <Page />
        </HandOff>
      </I18nextProvider>,
    );
    expect(mounts, "a search the assistant did not write changes nothing").toBe(1);

    searchStr = "?endpoint=public-bikes";
    rerender(
      <I18nextProvider i18n={i18n}>
        <HandOff>
          <Page />
        </HandOff>
      </I18nextProvider>,
    );
    expect(mounts, "a new endpoint is a new hand-off").toBe(2);
    expect(screen.getByText("public-bikes")).toBeInTheDocument();
  });

  it("names every hand-off the assistant may write", () => {
    // A name missing here is a hand-off that silently does not reopen its page (AG-73, AG-77).
    expect([...HAND_OFF]).toEqual(["edit", "delete", "grant", "draft", "space", "endpoint"]);
  });

  it.each(SUPPORTED_LOCALES)("leaves the page's own words alone in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    searchStr = "";
    const { container } = show();

    expect(screen.getByRole("heading", { name: i18n.t("explore.title") })).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});
