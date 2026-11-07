/**
 * T-1792: the login page meets the UI contract (UI-01, UI-11, UI-15, UI-16, UI-44, PF-50).
 *
 * The survey found nothing to replace here — the page is already the shared Card, Button and
 * Icon — so these cases hold it there, and hold the one thing a login page must never get wrong:
 * where it sends a person afterwards.
 */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { redirectTarget } from "../src/routes/LoginPage";
import { expectAxeClean, expectOneH1, LOCALES, OTHER_BRAND, renderRoute } from "./pageHarness";

describe("the login page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    // A signed-in case opens a project page, which remembers the project; each case starts clean.
    window.localStorage.clear();
    window.sessionStorage.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    // The keyboard case stands in for `location`; the cases after it need the real one.
    vi.unstubAllGlobals();
  });

  it("opens_under_one_h1_with_one_primary_action_and_no_axe_violation", async () => {
    const { container } = await renderRoute({ path: "/", identity: null });
    await screen.findByRole("button", { name: en.auth.signIn });
    await expectOneH1();
    expect(screen.getByText(en.auth.lead)).toBeInTheDocument();
    // One primary action on the page, which is the whole of a login page (UI-01).
    expect(screen.getAllByRole("button", { name: en.auth.signIn })).toHaveLength(1);
    await expectAxeClean(container);
  });

  it("the_sign_in_action_is_reached_and_used_from_the_keyboard", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, search: "", href: "http://localhost/" });
    await renderRoute({ path: "/", identity: null });
    const signIn = await screen.findByRole("button", { name: en.auth.signIn });
    signIn.focus();
    expect(signIn).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(assign).toHaveBeenCalled());
  });

  it("comes_back_only_to_this_origin", () => {
    // `redirect_to` is a query parameter, which is to say it is whatever the address bar holds.
    expect(redirectTarget("?redirect_to=/projects/helsinki/spaces")).toBe("/projects/helsinki/spaces");
    expect(redirectTarget("?redirect_to=https://evil.example/steal")).toBe("/");
    expect(redirectTarget("?redirect_to=//evil.example/steal")).toBe("/");
    expect(redirectTarget("?redirect_to=javascript:alert(1)")).toBe("/");
    expect(redirectTarget("")).toBe("/");
    // A backslash or a tab after the slash makes the browser read another host (F-UI3-007).
    expect(redirectTarget("?redirect_to=/%5Cevil.example")).toBe("/");
    expect(redirectTarget("?redirect_to=/%09/evil.example")).toBe("/");
  });

  it("sends_a_person_who_is_already_signed_in_on_to_where_they_were_going", async () => {
    // Behind the edge, Keycloak sends a person back to the address they opened, /login included
    // (T-2729): a Sign in button offered to somebody signed in is a step that does nothing.
    await renderRoute({ path: "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces" });
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces"));
    expect(screen.queryByRole("button", { name: en.auth.signIn })).toBeNull();
  });

  it("never_follows_an_address_off_this_origin_for_a_signed_in_person", async () => {
    await renderRoute({ path: "/login?redirect_to=%2F%2Fevil.example%2Fsteal" });
    await waitFor(() => expect(window.location.pathname).not.toBe("/login"));
    expect(window.location.href).not.toContain("evil.example");
  });

  it("keeps_no_identity_and_no_token_in_the_browser", async () => {
    await renderRoute({ path: "/", identity: null });
    await screen.findByRole("button", { name: en.auth.signIn });
    // The session is the cookie the Portal sets; nothing about it is copied into storage (UI-09).
    // What a viewer may keep in their own browser is how they like the UI, and it is prefixed.
    expect(Object.keys(window.localStorage).filter((key) => !key.startsWith("jc-"))).toEqual([]);
    expect(window.sessionStorage.length).toBe(0);
  });

  it("offers_the_language_before_the_sign_in_so_it_can_be_read_first", async () => {
    await renderRoute({ path: "/", identity: null });
    const signIn = await screen.findByRole("button", { name: en.auth.signIn });
    const language = screen.getByRole("button", { name: /language|jazyk|sprache/i });
    expect(
      language.compareDocumentPosition(signIn) & Node.DOCUMENT_POSITION_FOLLOWING,
      "the language switcher comes before the one action on the page",
    ).toBeTruthy();
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: "/", identity: null, locale });
      expect(await screen.findByRole("button", { name: i18n.t("auth.signIn") })).toBeInTheDocument();
      expect(screen.getByText(i18n.t("auth.lead"))).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByText(en.auth.lead)).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("paints_in_the_installations_own_colours_not_a_literal", async () => {
    const { container } = await renderRoute({ path: "/", identity: null, brand: OTHER_BRAND });
    await screen.findByRole("button", { name: en.auth.signIn });
    const literal = [...container.querySelectorAll<HTMLElement>("[style]")].filter((element) =>
      /#[0-9a-f]{3,8}|\brgba?\(/i.test(element.getAttribute("style") ?? ""),
    );
    expect(literal.map((element) => element.outerHTML.slice(0, 120))).toEqual([]);
  });

  it("names_the_organisation_from_the_branding_and_renders_it_as_text", async () => {
    const { container } = await renderRoute({
      path: "/",
      identity: null,
      brand: { ...OTHER_BRAND, organisation: "<img src=x onerror=alert(1)>" },
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });
});
