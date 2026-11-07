/**
 * T-2315: the measure behind the UI rules allow-list, in one place.
 *
 * `ui_rules.test.ts` reads the sources and fails on every line that breaks a rule; this is the
 * part of it a second test needs — the pattern each allow-listed rule is measured by, and what
 * an entry claims against what its file still holds. It lives here rather than in the test so
 * that `ui_rules_merge.test.ts` can measure a hypothetical allow-list without importing a suite.
 */

/** A source file as the rules read it. */
export interface Source {
  /** Path relative to `ui/`, as the allow-list names it. */
  path: string;
  text: string;
  /** A shared control may build what a page may not: that is what makes it the shared one. */
  shared: boolean;
}

export interface AllowEntry {
  /** Exactly how many lines of this file break the rule today. */
  lines: number;
  /** The task group that empties this file. */
  group: string;
  /** Why a line stays, when it is right as it is. */
  reason?: string;
}

/**
 * One rule's allowance: an entry per file it may still break, and `_max`, the ceiling on how
 * many files that is (T-2316).
 *
 * The ceiling is what the old `files:` total was, moved in here. It matters because every other
 * check measures the files the list already names: a NEW entry, sized honestly to the violations
 * it exempts, satisfies all of them. The ceiling is the one thing a new entry cannot satisfy on
 * its own, so listing a file instead of fixing it costs a second, named line in the same commit.
 * It never has to move when a file is cleaned out, which is what keeps two workers from
 * conflicting over it — the property T-2315 bought.
 */
export interface RuleAllowance {
  /** Optional in the type, required by the suite: a rule with none fails `overCeiling`. */
  _max?: number;
  [path: string]: AllowEntry | number | undefined;
}

export type Allowed = Record<string, RuleAllowance>;

/** The file entries of one rule: everything but the `_max` ceiling beside them. */
export function entriesOf(rule: RuleAllowance | undefined): [string, AllowEntry][] {
  return Object.entries(rule ?? {}).filter(
    (pair): pair is [string, AllowEntry] => pair[0] !== "_max" && typeof pair[1] === "object",
  );
}

/** The entry for one file under one rule, or nothing when the rule does not name it. */
export function entryOf(rule: RuleAllowance | undefined, path: string): AllowEntry | undefined {
  const found = rule?.[path];
  return typeof found === "object" ? found : undefined;
}

export interface OverCeiling {
  rule: string;
  /** The rule's `_max`, or null when it declares none at all. */
  ceiling: number | null;
  /** How many files it names. */
  files: number;
}

/**
 * Every rule that names more files than its ceiling allows — and every rule that declares no
 * ceiling, because a missing one would be the same exemption by another route.
 */
export function overCeiling(allow: Allowed): OverCeiling[] {
  return Object.entries(allow).flatMap(([rule, allowance]) => {
    const files = entriesOf(allowance).length;
    const ceiling = typeof allowance._max === "number" ? allowance._max : null;
    return ceiling === null || files > ceiling ? [{ rule, ceiling, files }] : [];
  });
}

/** The pattern behind each allow-listed rule, so a stale entry can be found by the same measure. */
export const RULE_OF: Record<string, RegExp> = {
  hand_made_control: /<(?:button|input|select|textarea|table)\b/,
  // A panel drawn by hand instead of `<Card>` (T-4888). A popover or a floating dock is a layer,
  // not a panel: `absolute` or `fixed` earlier in the same class string keeps it out.
  panel_is_a_card:
    /(?<!\b(?:absolute|fixed)\b[^"`]*)\brounded-(?:lg|xl) border border-border bg-surface(?![-\w])/,
  // A `p-*` class on a Card loses or wins against its own padding by CSS source order: the
  // `padding` prop is the one way to size it (T-4888). A `flush` card has no padding to fight.
  card_padding_is_a_prop: /<Card\b(?![^>]*\bflush\b)[^>]*\bclassName="[^"]*(?<![\w-])p-\d/,
  colour_is_a_token:
    /#[0-9a-fA-F]{3,8}\b(?![-\w])|\brgba?\(|\b(?:bg|text|border|ring|fill|stroke)-(?:red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone)-\d{2,3}\b/,
  size_is_on_the_scale:
    /\b(?:bg|text|border|p|px|py|m|mx|my|w|h|gap|rounded|shadow|min-w|max-w|min-h|max-h)-\[[^\]]+\]|style=\{\{/,
  asks_with_a_shared_dialog: /window\.confirm\(|window\.alert\(/,
  focus_is_not_stolen: /\bautoFocus\b/,
  check_is_not_suppressed: /@ts-ignore|@ts-expect-error|eslint-disable/,
};

/**
 * The rules that carry no allow-list at all, and may never gain one: every one of them is zero
 * across the whole UI, and an entry here would be a permanent exemption from a security rule.
 */
export const UNLISTABLE = new Set([
  "no_markup_is_built_from_a_string",
  "no_code_is_built_from_a_string",
  "no_value_is_cast_to_never",
  "a_new_tab_link_has_rel_noopener",
  "browser_storage_holds_preferences_only",
  "no_file_reaches_for_a_colour_family_the_theme_never_defined",
  "no_file_paints_with_half_of_a_token_name",
  "no_text_is_painted_in_the_raw_brand_colour",
  "no_class_names_a_tone_or_a_size_the_theme_never_defined",
]);

/** Every line of `file` that `pattern` matches, as `path:line`. */
export function hits(file: Source, pattern: RegExp): string[] {
  return [...file.text.matchAll(new RegExp(pattern.source, `${pattern.flags}g`))].map(
    (match) => `${file.path}:${file.text.slice(0, match.index).split("\n").length}`,
  );
}

/**
 * The lines that break `rule`, minus the ones the allow-list names for that file. A file whose
 * count has grown fails with the lines, because a file that is allowed 3 and now has 5 has two
 * new violations, not permission for five.
 */
export function breaches(allow: Allowed, rule: string, pattern: RegExp, files: Source[]): string[] {
  const allowed = allow[rule];
  return files.flatMap((file) => {
    const found = hits(file, pattern);
    const budget = entryOf(allowed, file.path)?.lines ?? 0;
    return found.length > budget ? found.slice(budget) : [];
  });
}

export interface StaleEntry {
  rule: string;
  path: string;
  allowed: number;
  found: number;
}

/**
 * Every entry that claims more than its file still holds — the half of the ratchet that refuses
 * a raise. A number written larger than the file is shows up here, so `lines` can only go up in
 * a change that really did add the violations, and that change is a diff on one file's object.
 */
export function staleEntries(allow: Allowed, files: Source[]): StaleEntry[] {
  return Object.entries(allow).flatMap(([rule, allowance]) =>
    entriesOf(allowance).flatMap(([path, entry]) => {
      const file = files.find((source) => source.path === path);
      const pattern = RULE_OF[rule];
      if (!file || !pattern) {
        return [];
      }
      const found = hits(file, pattern).length;
      return found < entry.lines ? [{ rule, path, allowed: entry.lines, found }] : [];
    }),
  );
}
