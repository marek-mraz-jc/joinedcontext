/**
 * UI-15, UI-16, UI-44 (T-1731): the rules every button on every page of the router keeps.
 *
 * The shared `Button` carries four variants and three sizes and, until these cases, which one a
 * page reached for was the author's taste: thirty-four hand-made buttons, pages with two primary
 * actions competing for the same answer, and a destructive action that was one click from done.
 * The rules are written down in `docs/Development/03-frontend-guidelines.md` §5; this file is
 * where they are held, over the whole router rather than over the pages a fixture happens to
 * open.
 */
import { waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { routerPaths } from "./gates";
import { jsonResponse, LOCALES, renderRoute } from "./pageHarness";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path: string) => readFileSync(join(UI, path), "utf8");

const PROJECT = "helsinki";

/**
 * Every page of the router, by the address a person reaches it at. `every_page_of_the_router_is_
 * here` holds this list against `router.tsx`, so a new route is a red test until its buttons are
 * checked too.
 */
const PAGES = [
  "/login",
  "/connect/bbsk/otvorene-data",
  `/projects/${PROJECT}/activity`,
  `/projects/${PROJECT}/grafana`,
  `/projects/${PROJECT}/health`,
  `/projects/${PROJECT}/approvals`,
  `/projects/${PROJECT}/approvals/chg-1a2b3c4d`,
  `/projects/${PROJECT}/mcp-elicitations/eli-0a1b2c3d4e5f6a7b`,
  "/mcp-confirmations/0a1b2c3d4e5f60718293a4b5c6d7e8f9",
  "/endpoints",
  "/organization",
  "/organization/settings",
  "/organization/members",
  "/organization/people",
  "/organization/projects",
  "/organization/health",
  "/organization/mcp-servers",
  "/connectors",
  "/ai-connectors",
  "/settings",
  "/organization/models",
  "/organization/groups/new",
  "/organization/data-access",
  `/projects/${PROJECT}/settings`,
  `/projects/${PROJECT}/settings/general`,
  `/projects/${PROJECT}/settings/members`,
  `/projects/${PROJECT}/settings/access`,
  `/projects/${PROJECT}/settings/danger`,
  `/projects/${PROJECT}/settings/service-accounts/new`,
  "/playground",
  `/projects/${PROJECT}/models`,
  `/projects/${PROJECT}/models/ovzdusie`,
  `/projects/${PROJECT}/models/ovzdusie/diagram`,
  `/projects/${PROJECT}/explore`,
  `/projects/${PROJECT}/ckan`,
  `/projects/${PROJECT}/mcp`,
  `/projects/${PROJECT}/knowledge`,
  `/projects/${PROJECT}/knowledge/web`,
  `/projects/${PROJECT}/knowledge/conversations`,
  "/catalogue",
  "/catalogue/ovzdusie",
  "/catalogue/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa",
  "/v/ovzdusie-board",
  "/f/ovzdusie-form",
  "/d/abcdefghijklmnopqrstuvwxyz234567",
  "/glossary",
  "/accessibility",
  `/projects/${PROJECT}/home`,
  `/projects/${PROJECT}/inbox`,
  `/projects/${PROJECT}/import`,
  `/projects/${PROJECT}/federation`,
  `/projects/${PROJECT}/spaces/complete`,
  `/projects/${PROJECT}/spaces/upload`,
  `/projects/${PROJECT}/spaces/ovzdusie`,
  `/projects/${PROJECT}/endpoints/air-quality`,
  `/projects/${PROJECT}/apps/mapa`,
  `/projects/${PROJECT}/apps/mapa/open`,
  `/projects/${PROJECT}/assistant`,
  `/projects/${PROJECT}/shared`,
  `/projects/${PROJECT}/workspaces`,
  `/projects/${PROJECT}/workspaces/new`,
  `/projects/${PROJECT}/secrets/new`,
  `/projects/${PROJECT}/workspaces/kopia/compare`,
  `/projects/${PROJECT}/workspaces/kopia/try-it`,
  `/projects/${PROJECT}/workspaces/kopia/bring-back`,
  // `/projects/{project}/{plural}`: every kind's own page, which is a view of its own.
  `/projects/${PROJECT}/spaces`,
  `/projects/${PROJECT}/endpoints`,
  `/projects/${PROJECT}/pipelines`,
  `/projects/${PROJECT}/policies`,
  `/projects/${PROJECT}/datasources`,
  `/projects/${PROJECT}/dashboards`,
  `/projects/${PROJECT}/dataproducts`,
  // The old Flows address only redirects to the home (T-6096).
  `/projects/${PROJECT}/flows`,
  `/projects/${PROJECT}/apps`,
  `/projects/${PROJECT}/syncsources`,
  `/projects/${PROJECT}/access`,
  `/projects/${PROJECT}`,
  // A kind's create and edit forms are pages at addresses of their own (T-2474).
  `/projects/${PROJECT}/policies/new`,
  `/projects/${PROJECT}/policies/verejne-citanie/edit`,
];

/** A person who may do everything, so no control is missing for want of a grant. */
const EDITOR = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor", "portal-approver"],
};

/** The caller's own copy, so the page that offers to discard one actually draws that button. */
const WORKSPACE = {
  name: "ws-air-quality",
  title: "Air quality rework",
  owner: "jana.kovacova@banskabystrica.sk",
  scope: { kind: "project" },
  expiresAt: "2026-04-01T00:00:00Z",
  previewState: "running",
};

/** One manifest of whatever kind the page asks for, so a list page is not empty. */
function answer(path: string): Response | undefined {
  // What the signed-in person may read (T-6411): nothing internal, as a new member sees it.
  if (path === "/api/v1/catalogue/readable") return jsonResponse({ datasets: [], unanswered: 0 });
  if (path === "/api/v1/catalogue") {
    const none = { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] };
    return jsonResponse({
      total: 1,
      page: 1,
      pageSize: 20,
      datasets: [{ name: "ovzdusie", title: "Ovzdušie", formats: ["CSV"], themes: ["ENVI"] }],
      facets: { ...none, theme: [{ value: "ENVI", label: "Environment", count: 1 }] },
      unavailable: [],
    });
  }
  if (path === "/api/v1/catalogue/datasets/ovzdusie") {
    return jsonResponse({
      name: "ovzdusie",
      title: "Ovzdušie",
      keywords: [],
      themes: [],
      spatial: [],
      catalogueUrl: "https://data.example.org/dataset/ovzdusie",
      resources: [{ name: "CSV", format: "CSV", url: "https://example.org/file.csv" }],
    });
  }
  if (path.endsWith("/workspaces")) {
    return jsonResponse({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [WORKSPACE] });
  }
  if (/\/api\/v1\/projects\/[^/]+$/.test(path)) {
    return jsonResponse({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Project",
      metadata: { name: PROJECT },
      status: { usage: { contextSpaces: { used: 1, limit: 4 } } },
    });
  }
  return undefined;
}

const open = (path: string) =>
  renderRoute({ path, identity: EDITOR, answer, permissions: { project: PROJECT, bootstrap: true, grants: [] } });

/**
 * The buttons of one variant inside the view, as `buttonClass` writes the variant: the class
 * token, not a hover shade. The shell around the page is not the view — the assistant's bubble
 * is the Portal's own affordance on every page, and an open dialog is a view of its own, counted
 * where it is opened.
 */
const of = (variant: "bg-primary" | "bg-danger") => {
  const view = document.querySelector("main") ?? document.body;
  // A list behind its routed form is hidden, so it is not what the view offers (T-2474).
  return [...view.querySelectorAll<HTMLElement>(`button.${variant}, a.${variant}`)].filter(
    (element) => element.closest("[hidden]") === null,
  );
};

/** What a person reads on the button, which is what "one primary" is counted over. */
const named = (element: HTMLElement) =>
  (element.textContent || element.getAttribute("aria-label") || "").trim();

/** The destructive buttons these cases actually clicked, so the check cannot pass on an empty page. */
const asked: string[] = [];

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the buttons of every page", () => {
  it("every_page_of_the_router_is_here", () => {
    const router = read("src/router.tsx");
    const paths = routerPaths(router);
    const missing = paths.filter((path) => {
      if (path === "/" || path === "/__gallery") return false;
      // `$param` stands for a value, a trailing `$` for the rest of the address (a splat); the
      // list carries one address per route.
      const pattern = new RegExp(
        `^${path.replace(/\/\$$/, "/.+").replace(/\$[a-zA-Z]+/g, "[^/]+")}$`,
      );
      return !PAGES.some((address) => pattern.test(address));
    });
    expect(missing, "a route with no address in PAGES: its buttons are checked by nothing").toEqual([]);
  });

  it.each(PAGES)("a_view_has_one_primary_button on %s", async (path) => {
    const { container } = await open(path);
    await waitFor(() => expect(container.querySelector("main, form")).toBeTruthy());
    // Distinct actions, not distinct buttons: the same create offered in the header and again
    // in the empty list is one action a person can take, which is the pattern of T-1381.
    const actions = [...new Set(of("bg-primary").map(named))];
    expect(actions, `${path} offers more than one primary action`).toHaveLength(
      Math.min(actions.length, 1),
    );
    // A dialog the page opens by itself is a view of its own, and the same rule holds in it.
    for (const dialog of document.querySelectorAll<HTMLElement>('[role="dialog"], [role="alertdialog"]')) {
      const inside = [
        ...new Set(
          [...dialog.querySelectorAll<HTMLElement>("button.bg-primary, a.bg-primary")].map(named),
        ),
      ];
      expect(inside, `a dialog on ${path} offers more than one primary action`).toHaveLength(
        Math.min(inside.length, 1),
      );
    }
  });

  it.each(PAGES)("a_destructive_button_asks_first on %s", async (path) => {
    const user = userEvent.setup();
    const { container } = await open(path);
    await waitFor(() => expect(container.querySelector("main, form")).toBeTruthy());
    // The page's own data, not only its shell: a destructive button sits on a row, and a page
    // still waiting for its list has none to offer.
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    for (const button of of("bg-danger")) {
      if (button.closest('[role="dialog"], [role="alertdialog"]')) {
        // Already the confirmation itself.
        continue;
      }
      // A page whose address opens a dialog (`/workspaces/new`, T-2749): the list under it cannot
      // be pressed while the dialog is up, and the same list is checked at its own address.
      const modal = document.querySelector('[role="dialog"]');
      if (modal && !modal.contains(button)) {
        continue;
      }
      await user.click(button);
      const confirmation = await waitFor(() =>
        document.querySelector('[role="dialog"], [role="alertdialog"]'),
      );
      expect(confirmation, `${path}: "${named(button)}" does what it does without asking`).toBeTruthy();
      asked.push(`${path}: ${named(button)}`);
      // Leave the page as it was for the next button.
      await user.keyboard("{Escape}");
    }
  });
  it("asked before every destructive button it found, and it found some", () => {
    // Without this the case above passes on a page that draws no destructive button at all.
    expect(asked.length, "no destructive button was exercised on any page").toBeGreaterThan(0);
  });
});

describe("what a button says", () => {
  /** Every label a `<Button>` takes straight from the catalogue, as `t("…")`. */
  function labelKeys(): string[] {
    const walk = (dir: string): string[] =>
      readdirSync(dir).flatMap((entry) => {
        const path = join(dir, entry);
        return statSync(path).isDirectory() ? walk(path) : path.endsWith(".tsx") ? [path] : [];
      });
    const keys = new Set<string>();
    for (const file of walk(join(UI, "src"))) {
      const text = readFileSync(file, "utf8");
      for (const match of text.matchAll(/<Button\b[^>]*>\s*\{t\("([^"]+)"/g)) {
        keys.add(match[1]);
      }
    }
    return [...keys];
  }

  /**
   * A label that says nothing about what will happen. "Send" is not among them: on a message
   * box it is the verb, and the thing it sends is the text the person just typed beside it.
   */
  const EMPTY = ["ok", "submit", "yes", "no", "done", "go", "action", "button"];

  it("a_button_label_is_a_verb_phrase_in_four_locales", async () => {
    const keys = labelKeys();
    expect(keys.length, "no button takes its label from the catalogue?").toBeGreaterThan(20);
    const bad: string[] = [];
    for (const locale of LOCALES) {
      await i18n.changeLanguage(locale);
      for (const key of keys) {
        const label = String(i18n.t(key));
        if (label === key || label.startsWith(`${key.split(".")[0]}.`)) {
          bad.push(`${locale}: ${key} is not in the catalogue`);
        } else if (EMPTY.includes(label.toLowerCase().replace(/[.!…]/g, ""))) {
          bad.push(`${locale}: ${key} reads "${label}", which says nothing about what happens`);
        }
      }
    }
    await i18n.changeLanguage("en");
    expect(bad, bad.join("\n")).toEqual([]);
  });
});

describe("what a button is, in the source", () => {
  const sources = (dir: string): string[] =>
    readdirSync(dir).flatMap((entry) => {
      const path = join(dir, entry);
      return statSync(path).isDirectory() ? sources(path) : path.endsWith(".tsx") ? [path] : [];
    });

  /**
   * Every `<button>` and `<Button>` in a file, with its attributes and its children.
   *
   * A regular expression cannot read the opening tag on its own: an attribute holds JSX of its
   * own (`icon={<Icon …/>}`), so the `>` that ends the tag is the first one outside every brace
   * and every quote. This walks the characters instead.
   */
  function elements(text: string): { attributes: string; body: string; line: number }[] {
    const found: { attributes: string; body: string; line: number }[] = [];
    for (const match of text.matchAll(/<(button|Button)(?=[\s/>])/g)) {
      const tag = match[1];
      let index = match.index + match[0].length;
      let depth = 0;
      let quote: string | null = null;
      let selfClosing = false;
      for (; index < text.length; index += 1) {
        const character = text[index];
        if (quote) {
          if (character === quote) quote = null;
        } else if (character === "/" && text[index + 1] === "/") {
          // A comment inside the opening tag, skipped whole. Without this an apostrophe in one
          // ("the field's own message", NewProject.tsx) opened a quote that never closed, and
          // the tag's attributes and body were read from the wrong place — a primary button
          // with a label was reported as an icon with no name (T-1731).
          index = text.indexOf("\n", index);
          if (index === -1) break;
        } else if (character === "/" && text[index + 1] === "*") {
          const end = text.indexOf("*/", index + 2);
          if (end === -1) break;
          index = end + 1;
        } else if (character === '"' || character === "'" || character === "`") {
          quote = character;
        } else if (character === "{") {
          depth += 1;
        } else if (character === "}") {
          depth -= 1;
        } else if (depth === 0 && character === ">") {
          selfClosing = text[index - 1] === "/";
          break;
        }
      }
      const attributes = text.slice(match.index + match[0].length, index - (selfClosing ? 1 : 0));
      const closes = selfClosing ? -1 : text.indexOf(`</${tag}>`, index);
      found.push({
        attributes,
        body: closes > 0 ? text.slice(index + 1, closes) : "",
        line: text.slice(0, match.index).split("\n").length,
      });
    }
    return found;
  }

  it("an_icon_button_has_a_name", () => {
    const unnamed: string[] = [];
    let iconOnlyButtons = 0;
    for (const file of sources(join(UI, "src"))) {
      const text = readFileSync(file, "utf8");
      for (const { attributes, body, line } of elements(text)) {
        // What is left once the icons and the comments are gone: a `<span>` that holds a label
        // is words, wherever a media query hides it, so a button with one is not icon-only.
        const words = body
          .replace(/<Icon\b[^>]*\/>/g, "")
          .replace(/\{\/\*[\s\S]*?\*\/\}/g, "")
          .replace(/<[^>]*>/g, "")
          .replace(/\s+/g, "");
        const drawsAnIcon = /<Icon\b/.test(body) || /\bicon=/.test(attributes) || /<svg\b/.test(body);
        if (words !== "" || !drawsAnIcon) continue;
        iconOnlyButtons += 1;
        const where = `${file.slice(UI.length + 1)}:${line}`;
        // A name for the screen reader and a tooltip for the pointer, and both are needed: the
        // name alone leaves a sighted person guessing at a glyph.
        if (!/aria-label[=\s]/.test(attributes)) unnamed.push(`${where} has no aria-label`);
        if (!/\btitle[=\s]/.test(attributes)) unnamed.push(`${where} has no tooltip`);
      }
    }
    expect(unnamed, unnamed.join("\n")).toEqual([]);
    // Without this the case passes when the walk finds nothing to check at all.
    expect(iconOnlyButtons, "no icon-only button in the whole UI?").toBeGreaterThan(2);
  });

  /** The `footer={…}` of every Dialog and ConfirmDialog, with the line it sits on. */
  function footers(text: string): { body: string; line: number }[] {
    const found: { body: string; line: number }[] = [];
    for (const match of text.matchAll(/footer=\{/g)) {
      let index = match.index + match[0].length - 1;
      let depth = 0;
      for (; index < text.length; index += 1) {
        if (text[index] === "{") depth += 1;
        else if (text[index] === "}") {
          depth -= 1;
          if (depth === 0) break;
        }
      }
      found.push({
        body: text.slice(match.index + match[0].length, index),
        line: text.slice(0, match.index).split("\n").length,
      });
    }
    return found;
  }

  /**
   * A label key that means "leave this alone": the way out a dialog offers first. The last
   * segment is the word — `form.cancel`, `drift.modal.close`, and `approvals.rejectCancel`,
   * where the cancel is spelled into the name of the thing it cancels.
   */
  const leavesIt = (key: string): boolean => {
    const word = key.split(".").pop() ?? key;
    return /^(cancel|close)$/.test(word) || /[a-z](Cancel|Close)$/.test(word);
  };

  it("a_dialog_offers_cancel_before_the_action", () => {
    const wrong: string[] = [];
    let checked = 0;
    for (const file of sources(join(UI, "src"))) {
      const text = readFileSync(file, "utf8");
      for (const { body, line } of footers(text)) {
        // The label of each button in the footer, in the order a person tabs through them: the
        // first catalogue key inside the button itself, so a label written as a condition
        // (`{preparing ? t("export.preparing") : t("export.download")}`) is read like any other
        // and a reason in an attribute is not mistaken for one.
        const labels = elements(body)
          .map(({ body: label }) => /t\("([^"]+)"/.exec(label)?.[1])
          .filter((key): key is string => Boolean(key));
        if (labels.length < 2) continue;
        checked += 1;
        const where = `${file.slice(UI.length + 1)}:${line}`;
        if (!leavesIt(labels[0])) {
          wrong.push(`${where} opens with "${labels[0]}" and not with the way out`);
        }
        if (leavesIt(labels[labels.length - 1])) {
          wrong.push(`${where} ends with "${labels[labels.length - 1]}": the action is not last`);
        }
      }
    }
    expect(wrong, wrong.join("\n")).toEqual([]);
    // Every dialog in the Portal, not the few a fixture happens to open.
    expect(checked, "no dialog footer with two buttons in the whole UI?").toBeGreaterThan(5);
  });

  it("a_destructive_action_is_never_what_a_dialog_opens_on", () => {
    // The security line of T-1731: a confirmation is a decision a person makes, not one that
    // `Enter` makes for them on a dialog that has just appeared. Nothing in a footer takes the
    // focus to itself.
    const focused: string[] = [];
    for (const file of sources(join(UI, "src"))) {
      const text = readFileSync(file, "utf8");
      for (const { body, line } of footers(text)) {
        if (/\bautoFocus\b/.test(body)) {
          focused.push(`${file.slice(UI.length + 1)}:${line} focuses a button in the footer`);
        }
      }
    }
    expect(focused, focused.join("\n")).toEqual([]);
  });

  it("a_button_target_is_24_px", () => {
    // WCAG 2.5.8: every size the shared button offers states its own height, and the smallest
    // is 24 px. A page that needs something smaller does not get it by writing its own button.
    // The heights are the control tokens (T-6093): the spacing unit shrank to 0.2rem, so a
    // numbered `h-*` no longer says what it measures.
    const button = read("src/components/ui/Button.tsx");
    const tokens = read("src/tokens.css");
    const sizes = [...button.matchAll(/^\s{2}(xs|sm|md|lg): "([^"]+)",/gm)];
    expect(sizes.map((size) => size[1])).toEqual(["xs", "sm", "md", "lg"]);
    for (const [, name, classes] of sizes) {
      const height = /\bh-(control(?:-xs|-lg)?)\b/.exec(classes);
      expect(height, `size ${name} states no control height`).toBeTruthy();
      const rem = new RegExp(`--portal-${height![1]}: ([\\d.]+)rem;`).exec(tokens);
      expect(rem, `--portal-${height![1]} is not in tokens.css`).toBeTruthy();
      expect(Number(rem![1]) * 16, `size ${name} is under the 24 px WCAG 2.5.8 asks for`).toBeGreaterThanOrEqual(24);
    }
  });
});
