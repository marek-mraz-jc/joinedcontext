import type { Page } from "@playwright/test";

/** One control a finger can hit, with the box it hits. */
export interface TouchTarget {
  size: string;
  what: string;
  /** The shorter side of the box, in CSS px. */
  edge: number;
}

/**
 * Every touch target on the page (inside the open dialog when one is open), measured as a finger
 * meets it (T-5526, T-5596). Inline links inside running text are exempt, as in WCAG 2.5.8; an
 * input inside its label takes the label's box; a control the person cannot see (visually hidden,
 * inert, or inside a closed `<details>`) is no target.
 */
export async function touchTargets(page: Page): Promise<TouchTarget[]> {
  return page.evaluate(() => {
    const scope: ParentNode =
      document.querySelector("dialog[open]") ?? document;
    const inline = (element: Element) => {
      if (element.tagName !== "A") return false;
      const parent = element.parentElement;
      // A link in a sentence: its block holds text beside it (WCAG 2.5.8 inline exception).
      return (
        parent !== null &&
        ["P", "LI", "SPAN", "DD", "TD", "SMALL"].includes(parent.tagName) &&
        (parent.textContent ?? "").trim().length >
          (element.textContent ?? "").trim().length + 3
      );
    };
    const seen = (element: Element) => {
      const box = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      // Visually hidden helpers (sr-only, clip 1 px) are no target; neither is what a closed
      // `<details>` holds, which Chromium lays out but skips (content-visibility).
      return (
        box.width > 2 &&
        box.height > 2 &&
        style.visibility !== "hidden" &&
        style.display !== "none" &&
        Number(style.opacity) > 0 &&
        element.checkVisibility()
      );
    };
    const label = (element: Element) =>
      `${element.tagName.toLowerCase()} "${(element.getAttribute("aria-label") ?? element.getAttribute("title") ?? element.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 40)}"`;
    return [
      ...scope.querySelectorAll(
        "button, a[href], select, textarea, summary, input:not([type=hidden]), [role=button], [role=tab], [role=checkbox], [role=switch], [role=menuitem], [role=option]",
      ),
    ]
      .filter(
        (element) =>
          seen(element) && !inline(element) && !element.closest("[inert]"),
      )
      .map((element) => {
        // An input inside its label takes the label's box: the label is what the finger hits.
        const hit =
          element.tagName === "INPUT" && element.closest("label")
            ? (element.closest("label") as Element)
            : element;
        const box = hit.getBoundingClientRect();
        return {
          size: `${Math.round(box.width)}x${Math.round(box.height)}`,
          what: label(element),
          edge: Math.min(box.width, box.height),
        };
      });
  });
}

/** The targets under a finger's 44 px, as "WxH tag "name"" lines. */
export async function smallTouchTargets(
  page: Page,
  finger = 44,
): Promise<string[]> {
  return [
    ...new Set(
      (await touchTargets(page))
        .filter((target) => target.edge < finger - 0.5)
        .map((target) => `${target.size} ${target.what}`),
    ),
  ];
}
