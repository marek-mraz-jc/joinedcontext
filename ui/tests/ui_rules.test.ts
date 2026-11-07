/**
 * T-1726: the UI rules as one test (UI-01, UI-16, PF-50).
 *
 * `components/ui/index.ts` says "nothing on a page styles a control by hand". This is what
 * enforces it. Each rule reads the sources and fails on every line that breaks it, except the
 * lines `ui_rules.allow.json` already names — today's measured state, which the per-file tasks of
 * the `ui-components`, `ui-forms`, `ui-pages` and `ui-parts` groups empty file by file. The
 * allow-list may only shrink: its totals are asserted, so a new violation cannot be waved through
 * by adding a line to it.
 *
 * The security rules carry no allow-list at all. They are zero today and this keeps them zero.
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  breaches as breachesOf,
  entriesOf,
  hits,
  overCeiling,
  RULE_OF,
  staleEntries,
  UNLISTABLE,
} from "./uiRules";
import type { Allowed, Source } from "./uiRules";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const allowPath = join(ui, "tests/ui_rules.allow.json");

const allow = JSON.parse(readFileSync(allowPath, "utf8")) as Allowed;

function sources(): Source[] {
  const out: Source[] = [];
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (/\.tsx?$/.test(entry.name))
        out.push({
          path: relative(ui, full),
          text: readFileSync(full, "utf8"),
          shared: full.includes("/components/ui/"),
        });
    }
  };
  walk(join(ui, "src"));
  return out;
}

/**
 * The numbered colour families `@theme` defines — `primary-500`, `neutral-200` — which are
 * tokens here and resolve through `--portal-*`. Read from the stylesheet rather than listed
 * twice, so adding a family to the theme does not need this file edited.
 */
const THEMED = new Set(
  [...readFileSync(join(ui, "src/index.css"), "utf8").matchAll(/--color-([a-z]+)-\d{1,3}:/g)].map(
    (match) => match[1],
  ),
);

/**
 * The semantic colour names `@theme` defines whole: `fg`, `fg-muted`, `surface-subtle`, `danger`.
 * Read from the same file as `THEMED`, so a name added to the theme needs no edit here.
 */
const SEMANTIC = new Set(
  [...readFileSync(join(ui, "src/index.css"), "utf8").matchAll(/--color-([a-z-]+):/g)].map(
    (match) => match[1],
  ),
);

/**
 * The tails of those names — `muted` out of `fg-muted` and `surface-muted`, `subtle` out of
 * `surface-subtle` — minus any that is a whole name in its own right (`fg` is, so it is not a
 * tail). A tail on its own is the shape of the mistake: `text-muted` for `text-fg-muted`.
 * Tailwind emits no rule for a name the theme never defined, so the class is silently nothing
 * and the text renders at full foreground colour. Seventeen hints across four pages did.
 */
const TAILS = new Set(
  [...SEMANTIC]
    .flatMap((name) =>
      name
        .split("-")
        .map((_, index, parts) => parts.slice(index + 1).join("-"))
        .filter(Boolean),
    )
    .filter((tail) => !SEMANTIC.has(tail)),
);

/** Any `bg-rose-500`-shaped class whose family `@theme` never defined. */
const STOCK_PALETTE = new RegExp(
  `\\b(?:bg|text|border|ring|fill|stroke|from|via|to|accent|decoration|outline|shadow|divide)-([a-z]+)-\\d{2,3}\\b`,
  "g",
);

function stockColours(file: Source): string[] {
  return [...file.text.matchAll(STOCK_PALETTE)]
    .filter((match) => !THEMED.has(match[1]))
    .map((match) => `${file.path}:${file.text.slice(0, match.index).split("\n").length}`);
}

const all = sources();
const pages = all.filter((file) => !file.shared);

/** The lines of `files` that break `rule`, minus the ones the allow-list names (see `uiRules`). */
function breaches(rule: string, pattern: RegExp, files: Source[] = pages): string[] {
  return breachesOf(allow, rule, pattern, files);
}

describe("what a page may not do by hand", () => {
  it("a_page_does_not_hand_make_a_button_input_select_textarea_or_table", () => {
    expect(breaches("hand_made_control", /<(?:button|input|select|textarea|table)\b/)).toEqual([]);
  });

  it("a_panel_is_the_shared_card", () => {
    expect(breaches("panel_is_a_card", RULE_OF.panel_is_a_card)).toEqual([]);
  });

  it("a_card_is_padded_by_its_prop", () => {
    // The rule bites on the shape it names and lets a flush card or the prop through.
    expect(RULE_OF.card_padding_is_a_prop.test('<Card className="flex p-3">')).toBe(true);
    expect(RULE_OF.card_padding_is_a_prop.test('<Card flush className="px-3 p-8">')).toBe(false);
    expect(RULE_OF.card_padding_is_a_prop.test('<Card padding="sm" className="gap-2">')).toBe(false);
    expect(RULE_OF.panel_is_a_card.test('className="absolute z-50 rounded-lg border border-border bg-surface"')).toBe(false);
    expect(RULE_OF.panel_is_a_card.test('className="rounded-lg border border-border bg-surface-subtle"')).toBe(false);
    expect(breaches("card_padding_is_a_prop", RULE_OF.card_padding_is_a_prop)).toEqual([]);
  });

  it("a_number_or_a_date_is_written_in_the_persons_language", () => {
    // `toLocaleString()` with no locale, or an `Intl` formatter with none, speaks the browser's
    // language: a Slovak page read 9/27/2026 and 1,234.5 (UI-15, T-3278). Every file, shared ones
    // too: a shared control is where the most numbers are written.
    expect(
      breaches(
        "locale_from_the_portal",
        /\.toLocale(?:Date|Time)?String\(\s*\)|new Intl\.(?:NumberFormat|DateTimeFormat|RelativeTimeFormat)\(\s*\)/,
        all,
      ),
    ).toEqual([]);
  });

  it("an_action_does_not_wait_for_a_hover", () => {
    // A phone has no hover (T-3279): a control drawn only under `group-hover` is one a finger never
    // finds. Show it, or show it on focus as well.
    expect(
      breaches("action_shown_on_hover_only", /\b(?:opacity-0|invisible|hidden)\b[^"'`]*\bgroup-hover:(?!.*group-focus)/, all),
    ).toEqual([]);
  });

  it("a_colour_is_a_token", () => {
    expect(
      breaches(
        "colour_is_a_token",
        /#[0-9a-fA-F]{3,8}\b(?![-\w])|\brgba?\(|\b(?:bg|text|border|ring|fill|stroke)-(?:red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone)-\d{2,3}\b/,
      ),
    ).toEqual([]);
  });

  it("no_file_reaches_for_a_colour_family_the_theme_never_defined", () => {
    // Everywhere, shared controls included: a shared control may build a control by hand — that
    // is what makes it the shared one — but nothing in the UI may paint in a colour no theme and
    // no installation's brand can reach. `Badge` carried `bg-purple-500` for months because this
    // rule skipped its whole folder, and `purple` was missing from the pattern besides.
    // `primary` and `neutral` are the two numbered families today; the guard is that the read
    // worked at all, not how many there are.
    expect(THEMED.size, "no themed colour families were read from index.css").toBeGreaterThan(1);
    expect(all.flatMap(stockColours)).toEqual([]);
  });

  it("no_file_paints_with_half_of_a_token_name", () => {
    // `text-muted` where the theme defines `fg-muted`: Tailwind emits nothing for it, so the
    // class does nothing and the hint renders at full foreground weight, looking exactly like
    // the label above it. Seventeen of them across AppGenerator, AppsCatalog, EndpointPreview
    // and Instantiate, and no rule saw them because the existing one only reads numbered
    // families. Shared controls included: the mistake is as easy to make there.
    expect(TAILS.size, "no tails were derived from the theme's own names").toBeGreaterThan(1);
    // `(?<![\w-])` so the prefix is the start of the class and not the middle of a longer one:
    // `ring-border-focus` names the real `border-focus` token and must not read as `border-` +
    // the tail `focus`.
    const pattern = new RegExp(
      `(?<![\\w-])(?:bg|text|border|ring|fill|stroke|divide|outline)-(?:${[...TAILS].join("|")})\\b`,
      "g",
    );
    expect(
      all.flatMap((file) =>
        [...file.text.matchAll(pattern)].map(
          (match) =>
            `${file.path}:${file.text.slice(0, match.index).split("\n").length} ${match[0]}`,
        ),
      ),
    ).toEqual([]);
  });

  it("no_class_names_a_tone_or_a_size_the_theme_never_defined", () => {
    // Tailwind emits nothing for a class the theme never declared, so the class is silently no
    // class at all and the element renders at whatever it inherited. `text-h3` drew the activity
    // counters at body size beside their own captions, and `text-warning-fg` — a name the theme
    // has for `danger` and `primary` but not for `warning` — left two schema diagnostics in the
    // ordinary foreground colour (T-2422, UI-01, UI-30). The three rules above read the shape of
    // a stock palette, a numbered family and a tail of a real name; none of them reads a name
    // the theme simply does not have.
    //
    // What a prefix may take: a colour or a size `@theme` declares, a numbered family, or one of
    // Tailwind's own words for that prefix — listed here, because that list is short and a class
    // outside it is the mistake this rule is for.
    const OWN: Record<string, string[]> = {
      text: ["left", "center", "right", "justify", "start", "end", "nowrap", "wrap", "balance",
        "pretty", "ellipsis", "clip", "transparent", "current", "inherit",
        "xs", "sm", "base", "lg", "xl", "2xl", "3xl", "4xl", "5xl"],
      bg: ["transparent", "current", "inherit", "none", "cover", "contain", "fixed", "local",
        "scroll", "center", "top", "bottom", "left", "right", "repeat", "no-repeat", "clip",
        "origin", "auto"],
      border: ["transparent", "current", "inherit", "solid", "dashed", "dotted", "double",
        "hidden", "none", "collapse", "separate", "spacing", "x", "y", "t", "r", "b", "l", "s", "e"],
      ring: ["transparent", "current", "inherit", "inset", "offset"],
      // Not `fill-` or `stroke-`: a map style's paint properties are spelled the same way
      // (`fill-color`, `fill-opacity` in MapLibreView) and they are object keys, not classes.
      // `no_file_reaches_for_a_colour_family_the_theme_never_defined` already covers those two
      // prefixes for the shape that matters, a stock palette colour.
    };
    const sizes = new Set(
      [...readFileSync(join(ui, "src/index.css"), "utf8").matchAll(/--text-([a-z0-9-]+):/g)]
        .map((match) => match[1])
        .filter((name) => !name.endsWith("--line-height")),
    );
    const unknown = all.flatMap((file) =>
      Object.entries(OWN).flatMap(([prefix, own]) =>
        [...file.text.matchAll(new RegExp(`(?<![\\w-])${prefix}-([a-z][a-z0-9-]*)(?![\\w-])`, "g"))]
          .filter(([, name]) => {
            if (SEMANTIC.has(name) || own.includes(name)) return false;
            if (prefix === "text" && sizes.has(name)) return false;
            // `primary-500`, `neutral-0`: the numbered families are their own rule above.
            const numbered = /^([a-z]+)-\d{1,3}$/.exec(name);
            if (numbered && THEMED.has(numbered[1])) return false;
            // `border-b-2`, `border-x-4`: a width on one side, which is a size and not a tone.
            return !(prefix === "border" && /^[xytrbl]-\d+$/.test(name));
          })
          .map(
            (match) =>
              `${file.path}:${file.text.slice(0, match.index).split("\n").length} ${match[0]}`,
          ),
      ),
    );
    expect(sizes.size, "no type sizes were read from index.css").toBeGreaterThan(1);
    expect(unknown).toEqual([]);
  });

  it("a_size_is_on_the_scale", () => {
    expect(
      breaches(
        "size_is_on_the_scale",
        /\b(?:bg|text|border|p|px|py|m|mx|my|w|h|gap|rounded|shadow|min-w|max-w|min-h|max-h)-\[[^\]]+\]|style=\{\{/,
      ),
    ).toEqual([]);
  });

  it("nothing_asks_with_window_confirm", () => {
    expect(breaches("asks_with_a_shared_dialog", /window\.confirm\(|window\.alert\(/)).toEqual([]);
  });

  it("focus_is_not_stolen_on_arrival", () => {
    expect(breaches("focus_is_not_stolen", /\bautoFocus\b/)).toEqual([]);
  });

  it("a_check_is_not_suppressed", () => {
    expect(breaches("check_is_not_suppressed", /@ts-ignore|@ts-expect-error|eslint-disable/)).toEqual(
      [],
    );
  });
});

/** A `<pre>` opening tag, attributes in braces included. */
const PRE_TAG = /<pre\b(?:[^>{}]|\{(?:[^{}]|\{[^{}]*\})*\})*>/g;

/** Whether a `<pre>` tag can scroll: an overflow class, or a class string it does not spell out. */
function scrollingPre(tag: string): boolean {
  const open = tag.match(/<pre\b(?:[^>{}]|\{(?:[^{}]|\{[^{}]*\})*\})*>/)?.[0] ?? "";
  return /\boverflow-(?:[xy]-)?(?:auto|scroll)\b|\bclassName=\{/.test(open);
}

describe("what nothing in the UI may do", () => {
  // No allow-list below this line: every one of these is zero today.
  const everywhere = (pattern: RegExp): string[] => all.flatMap((file) => hits(file, pattern));

  it("no_markup_is_built_from_a_string", () => {
    expect(everywhere(/dangerouslySetInnerHTML|\.innerHTML\s*=|insertAdjacentHTML|document\.write/)).toEqual(
      [],
    );
  });

  it("no_value_is_cast_to_never", () => {
    // T-1488: `as never` fits any value into any slot, so a body the API renamed or a search the
    // route never declared type-checks until dev. A mismatch is fixed where it is (the route's
    // annotation, the route's search schema), and the generated types carry it here.
    expect(everywhere(/[\w)\]}]\s+as\s+never\b/)).toEqual([]);
  });

  it("an_error_line_and_a_dialog_are_the_shared_ones", () => {
    // T-3281: a red `<p role="alert">` came in six sizes and two reds, one of them the white of
    // text on a red background, and said "error" in colour alone: it is `InlineError`. A
    // `role="dialog"` on a page was a panel that called itself modal: it is `Dialog` or
    // `ConfirmDialog`, which move the focus and trap it.
    const handMade = pages.flatMap((file) =>
      [...file.text.matchAll(/<p\b[^<>]*\brole="alert"[^<>]*>|\brole="(?:alert)?dialog"/gs)].map(
        (tag) => `${file.path}:${file.text.slice(0, tag.index).split("\n").length}`,
      ),
    );
    expect(handMade).toEqual([]);
  });

  it("a_scrolling_block_is_the_shared_code_block", () => {
    // T-5014: a `<pre>` that scrolls hid its overflow from anyone without a mouse (axe
    // scrollable-region-focusable on twenty pages). `CodeBlock` is a named tab stop; a class held
    // in a variable may scroll too, so it counts.
    expect(scrollingPre('<pre className="max-h-40 overflow-auto font-mono">{x}</pre>')).toBe(true);
    expect(scrollingPre('<pre\n  data-testid="x"\n  className={pre}\n>')).toBe(true);
    expect(scrollingPre('<pre className="whitespace-pre-wrap [overflow-wrap:anywhere]">')).toBe(false);
    const bare = all
      .filter((file) => !file.path.endsWith("components/ui/CodeBlock.tsx"))
      .flatMap((file) =>
        [...file.text.matchAll(PRE_TAG)]
          .filter((tag) => scrollingPre(tag[0]))
          .map((tag) => `${file.path}:${file.text.slice(0, tag.index).split("\n").length}`),
      );
    expect(bare).toEqual([]);
  });

  it("no_code_is_built_from_a_string", () => {
    expect(everywhere(/\beval\(|new Function\(/)).toEqual([]);
  });

  it("a_new_tab_link_has_rel_noopener", () => {
    // Per element, not per file: one link with `rel` does not cover the next one without it.
    const open = all.flatMap((file) =>
      [...file.text.matchAll(/<(?:a|Link|area)\b[^>]*>/gs)]
        // `noreferrer` implies `noopener` in every browser that understands it, so either does.
        .filter(
          (tag) =>
            tag[0].includes('target="_blank"') &&
            !/rel="[^"]*(?:noopener|noreferrer)/.test(tag[0]),
        )
        .map((tag) => `${file.path}:${file.text.slice(0, tag.index).split("\n").length}`),
    );
    expect(open).toEqual([]);
    // `window.open` cannot carry `rel` at all, so it is not used.
    expect(everywhere(/window\.open\(/)).toEqual([]);
  });

  it("no_text_is_painted_in_the_raw_brand_colour", () => {
    // `text-primary` and `text-accent` are the installation's brand colour exactly as it was
    // configured, and a brand colour is chosen to be seen on a page, not read on one: the
    // default amber accent is 2.15:1 as text on white and a pale civic primary 1.67:1 (T-2323,
    // UI-30). The derived tones are what text takes — `text-primary-soft-fg` for a link, the
    // status tones for a status — because they are mixed towards the ink and clear 4.5:1 for
    // every brand `token_contrast.test.ts` measures. A brand colour stays a background
    // (`bg-primary`), which carries its own foreground token.
    expect(
      everywhere(/(?<![\w-])(?:text|fill|stroke|decoration)-(?:primary|secondary|accent)(?![\w-])/),
    ).toEqual([]);
  });

  it("browser_storage_holds_preferences_only", () => {
    // What a viewer may keep in their own browser: how they like the UI, never platform state
    // and never anything a session could be resumed from (UI-09, PF-50).
    // `jc-diagram-layout`: where a person moved boxes on a model's canvas (T-6094), a view preference.
    const keys = new Set(["jc-lang", "jc-theme", "jc-project", "jc-advanced", "jc-diagram-layout"]);
    const used = all.flatMap((file) =>
      [...file.text.matchAll(/(?:local|session)Storage\.\w+\(\s*("[^"]*"|`[^`]*`|[A-Z_]+)/g)].map(
        (match) => ({
          where: `${file.path}:${file.text.slice(0, match.index).split("\n").length}`,
          key: match[1].replace(/["`]/g, ""),
        }),
      ),
    );
    expect(used.length).toBeGreaterThan(0);
    expect(used.filter(({ key }) => !keys.has(key) && !/^[A-Z_]+$/.test(key))).toEqual([]);
  });
});

describe("the allow-list", () => {
  /**
   * The ratchet used to be six literal totals — one line naming every rule's file and line count
   * for the whole repository. Four workers clean the `ui-*` groups at once, and on 2026-09-20
   * that one line blocked three batches in a row while `ui_rules.allow.json` merged cleanly
   * beside it every time: the conflict was never about code, only about two people having
   * counted different halves of the same shrinking set (T-2315).
   *
   * It is gone, and nothing it enforced went with it. The ratchet is per file and runs in both
   * directions: `breaches` fails every line beyond what a file's entry allows, and
   * `no_entry_survives_the_file_it_was_written_for` fails an entry larger than its file still
   * is. So a number can only be raised by a change that really does add the violations — and
   * that change is a diff on that one file's object, naming the rule and the file, which is a
   * louder thing to read in review than a total moving from 73 to 74. Two workers cleaning two
   * files now edit two different JSON objects, and git merges them.
   */
  it("a_rule_nobody_wrote_cannot_be_allow_listed_into_existence", () => {
    // Every rule the allow-list names has a pattern here, so an entry can always be measured
    // against the file it was written for. A rule with no pattern would be an entry nothing
    // checks, which is how a permanent exemption gets in.
    expect(Object.keys(allow).filter((rule) => !(rule in RULE_OF))).toEqual([]);
    // And the security rules below carry no allow-list at all: they stay unlistable.
    expect(Object.keys(allow).filter((rule) => UNLISTABLE.has(rule))).toEqual([]);
  });

  it("no_rule_names_more_files_than_its_ceiling_allows", () => {
    // The half of the ratchet a new entry would otherwise walk past (T-2316): every other check
    // measures the files the list already names, so listing one more file — honestly sized to
    // the violations it exempts — satisfies all of them. `_max` is the ceiling the old `files:`
    // total was, and raising it is a named one-line diff in the same commit as the new entry.
    expect(
      overCeiling(allow),
      "fix the file, or raise this rule's _max in the same commit and say why",
    ).toEqual([]);
  });

  it("every_entry_names_a_file_that_exists_and_the_group_that_empties_it", () => {
    const known = new Set(pages.map((file) => file.path));
    const groups = new Set(["ui-components", "ui-forms", "ui-pages", "ui-parts"]);
    for (const [rule, allowance] of Object.entries(allow)) {
      for (const [path, entry] of entriesOf(allowance)) {
        expect(known, `${rule} allows ${path}, which no source file is`).toContain(path);
        expect(entry.lines, `${rule}/${path}`).toBeGreaterThan(0);
        expect(groups, `${rule}/${path} names no group`).toContain(entry.group);
      }
    }
  });

  it("no_entry_survives_the_file_it_was_written_for", () => {
    // A file cleaned below its entry is an entry to lower or delete, and this says which and to
    // what. It is also the half of the ratchet that refuses a raise: a number written larger
    // than the file is fails here, so `lines` can only go up when the violations really did.
    expect(staleEntries(allow, pages), "lower or delete these").toEqual([]);
  });
});

