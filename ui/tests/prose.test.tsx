/**
 * T-2773 (UI-84, AP-53): an answer is Markdown and reads as its lists, tables and links; the
 * model's text never becomes markup, a link is checked before it is one, and a long entity id is
 * shortened with a button that copies it whole.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { Prose, blocksOf, shortId } from "../src/pages/apps/Prose";
import { expectNoAxeViolations } from "./page_contract";

function prose(text: string, onOpenLink?: (href: string) => void) {
  return render(
    <I18nextProvider i18n={i18n}>
      <Prose text={text} onOpenLink={onOpenLink} />
    </I18nextProvider>,
  );
}

const URN = "urn:ngsi-ld:Event:hel.fi:helsinki-events:helsinki-af5kileoqi";

describe("an answer's Markdown", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("reads headings, paragraphs, both lists, code and a table as blocks", () => {
    expect(
      blocksOf(
        "## Events\nToday there are two.\nBoth free.\n\n- Plasma Online Art\n- Jazz\n  in the park\n\n1. first\n2. second\n\n```json\n{\"a\": 1}\n```\n| Name | When |\n|---|:--:|\n| Jazz | today |\n| A \\| B | later |",
      ),
    ).toEqual([
      { kind: "heading", text: "Events" },
      { kind: "paragraph", text: "Today there are two.\nBoth free." },
      { kind: "list", ordered: false, items: ["Plasma Online Art", "Jazz\nin the park"] },
      { kind: "list", ordered: true, items: ["first", "second"] },
      { kind: "code", text: "{\"a\": 1}" },
      { kind: "table", head: ["Name", "When"], rows: [["Jazz", "today"], ["A | B", "later"]] },
    ]);
  });

  it("keeps an empty answer, a lone pipe and an unclosed fence from breaking the reading", () => {
    expect(blocksOf("")).toEqual([]);
    expect(blocksOf("a | b")).toEqual([{ kind: "paragraph", text: "a | b" }]);
    expect(blocksOf("```\nhalf")).toEqual([{ kind: "code", text: "half" }]);
  });

  it("draws a table inside its own scroller, with column headers", () => {
    prose("| Station | Free bikes |\n|---|---|\n| Töölö | 0 |");
    const table = screen.getByRole("table");
    expect(table.parentElement).toHaveClass("overflow-x-auto");
    expect(within(table).getAllByRole("columnheader").map((cell) => cell.textContent)).toEqual([
      "Station",
      "Free bikes",
    ]);
    expect(within(table).getByRole("cell", { name: "Töölö" })).toBeInTheDocument();
  });

  it("puts the model's markup on the screen as text, never into the page", () => {
    const { container } = prose("<img src=x onerror=alert(1)> **<b>bold</b>** [x](javascript:alert(1))");
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(container.querySelector("strong")?.textContent).toBe("<b>bold</b>");
  });

  it("opens a Portal link in place and leaves one outside the Portal to a new tab", async () => {
    const opened = vi.fn();
    prose("Open [all 98 in Explore](/projects/helsinki/entities?type=Event) or https://hel.fi/events.", opened);
    await userEvent.click(screen.getByRole("link", { name: "all 98 in Explore" }));
    expect(opened).toHaveBeenCalledWith("/projects/helsinki/entities?type=Event");
    const outside = screen.getByRole("link", { name: /https:\/\/hel\.fi\/events/ });
    expect(outside).toHaveAttribute("href", "https://hel.fi/events");
    expect(outside).toHaveAttribute("target", "_blank");
  });

  // F-R2UI2-004: text the model read (a dataset, a web page) can write a label that hides its target.
  it("names the host beside a labelled link that leaves the Portal, and only there", () => {
    const { container } = prose("See [the Portal's endpoint](https://evil.example/x), [hel.fi](https://hel.fi/a), [hel.fi](https://el.fi/login) and [Explore](/projects/p/entities).");
    const after = (name: RegExp) => screen.getByRole("link", { name }).nextSibling?.textContent ?? "";
    expect(after(/the Portal's endpoint/)).toBe(" (evil.example)");
    const [named, hiding] = screen.getAllByRole("link", { name: /^hel\.fi/ });
    expect(named.nextSibling?.textContent).toBe(", ");
    // A label that only contains the host as part of a word does not name it.
    expect(hiding).toHaveAttribute("href", "https://el.fi/login");
    expect(hiding.nextSibling?.textContent).toBe(" (el.fi)");
    expect(after(/^Explore$/)).toBe(".");
    expect(container.textContent).toContain("(evil.example)");
  });

  it("shortens a long entity id, keeps it whole for a screen reader, and copies it whole", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = prose(`The event ${URN}. Short: urn:ngsi-ld:Event:x:y:z`);
    expect(shortId(URN)).toBe("urn:ngsi-ld:Event:…:helsinki-af5kileoqi");
    expect(shortId(`urn:ngsi-ld:Event:hel.fi:x:${"a".repeat(30)}b`)).toBe(`urn:ngsi-ld:Event:…${"a".repeat(19)}b`);
    expect(container.textContent).toContain(`${shortId(URN)}`);
    expect(container.querySelector(".sr-only")?.textContent).toBe(URN);
    expect(container.textContent).toContain("urn:ngsi-ld:Event:x:y:z");
    // The sentence's full stop stays the sentence's.
    expect(container.textContent).toContain(".");
    const copy = screen.getByRole("button", { name: en.agentRun.prose.copyId });
    await userEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith(URN);
    expect(await screen.findByRole("button", { name: en.agentRun.prose.copied })).toBeInTheDocument();
    expect(screen.getAllByRole("button")).toHaveLength(1);
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    prose("## Events\n\n- Jazz\n\n| Station | Free bikes |\n|---|---|\n| Töölö | 0 |\n\n```json\n{}\n```");
    await expectNoAxeViolations(document.body);
  });
});
