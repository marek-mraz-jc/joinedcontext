/**
 * T-3271: what changed, for the people who use the Portal. A dot on Help until the list is read,
 * the entries newest first in the person's language, and the dot gone once read.
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import { HelpMenu } from "../src/components/HelpMenu";
import { WHATS_NEW, isUnread } from "../src/whatsNew";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";

/** Help reads the page it is on, so it is drawn inside the router, on a page with no help of its own. */
const show = () => renderPage(<HelpMenu />, { answer: () => undefined, path: "/glossary" });

describe("what's new (T-3271)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });
  afterEach(() => localStorage.clear());

  it("the entries are newest first, each dated, and every one has words in every locale", () => {
    const days = WHATS_NEW.map((entry) => entry.date);
    expect(days).toEqual([...days].sort().reverse());
    expect(new Set(WHATS_NEW.map((entry) => entry.key)).size).toBe(WHATS_NEW.length);
    for (const entry of WHATS_NEW) {
      expect(entry.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      const words = (en.whatsNew.entries as Record<string, { title: string; body: string }>)[entry.key];
      expect(words?.title, entry.key).toBeTruthy();
      expect(words?.body, entry.key).toBeTruthy();
      expect((sk.whatsNew.entries as Record<string, { title: string }>)[entry.key]?.title, entry.key).not.toBe(words.title);
    }
  });

  it("help says how many changes are new until the list is read, then nothing", async () => {
    show();
    const help = await screen.findByRole("button", { name: `Help, ${WHATS_NEW.length} new changes` });
    await userEvent.click(help);
    await userEvent.click(await screen.findByRole("menuitem", { name: `What's new (${WHATS_NEW.length} new)` }));
    const dialog = await screen.findByRole("dialog", { name: en.whatsNew.title });
    const first = WHATS_NEW[0];
    expect(within(dialog).getByRole("heading", { name: (en.whatsNew.entries as Record<string, { title: string }>)[first.key].title })).toBeInTheDocument();
    expect(within(dialog).getAllByText(en.whatsNew.new).length).toBe(WHATS_NEW.length);
    await expectNoViolations(dialog);
    await userEvent.keyboard("{Escape}");
    expect(await screen.findByRole("button", { name: "Help" })).toBeInTheDocument();
    expect(localStorage.getItem("jc.whatsNewSeen")).toBe(first.date);
  });

  it("an entry is new only when it is newer than the last one read", () => {
    const entry = { key: "x", date: "2026-10-07" };
    expect(isUnread(entry, undefined)).toBe(true);
    expect(isUnread(entry, "2026-10-06")).toBe(true);
    expect(isUnread(entry, "2026-10-07")).toBe(false);
  });

  it("reads in Slovak", async () => {
    await i18n.changeLanguage("sk");
    show();
    await userEvent.click(await screen.findByRole("button", { name: /^Pomocník/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /^Čo je nové/ }));
    expect(await screen.findByRole("dialog", { name: sk.whatsNew.title })).toBeInTheDocument();
  });
});
