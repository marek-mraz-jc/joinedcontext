/**
 * T-3847: the Portal's words, held to the UI writing rules (docs User-Guide/ui-writing.md).
 *
 * Every string of every locale and every disabled control in the source is checked by the rules
 * a machine can judge (`human_text.ts`). What breaks a rule today is recorded in
 * `human_text.allow.json` with a reason, the work list of the walks T-3848…T-3852: a new break
 * is a red build, and an entry that no longer breaks anything is too, so the list only shrinks.
 * `human_ui_walk.json` records who walked each module; once `enforce` is set, a module with
 * controls and no walk is red.
 *
 *     HUMAN_TEXT_RECORD=1 pnpm vitest run tests/gate_human_text.test.ts   # rewrites the list
 */
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  hasNextStep,
  leaves,
  missingKeys,
  silentDisabled,
  textProblems,
} from "./human_text";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const LOCALES = ["en", "sk", "cs", "de"] as const;
const json = <T>(path: string): T =>
  JSON.parse(readFileSync(join(ui, path), "utf8")) as T;

interface Allowed {
  $comment: string[];
  strings: Record<string, { rules: string[]; reason: string }>;
  disabled: Record<string, { count: number; reason: string }>;
}
interface Walk {
  $comment: string[];
  enforce: boolean;
  modules: Record<
    string,
    { walkedBy: string; date: string; screenshots: string[] } | null
  >;
}

function walk(directory: string): string[] {
  return readdirSync(join(ui, directory), { withFileTypes: true }).flatMap(
    (entry) => {
      const path = `${directory}/${entry.name}`;
      if (entry.isDirectory()) return walk(path);
      return /\.tsx$/.test(entry.name) ? [path] : [];
    },
  );
}

const locales = Object.fromEntries(
  LOCALES.map((l) => [l, leaves(json(`src/locales/${l}.json`))]),
) as Record<(typeof LOCALES)[number], Record<string, string>>;

/** What breaks a rule now: the English text, which every locale translates key for key. */
function stringBreaks(): Allowed["strings"] {
  const found: Allowed["strings"] = {};
  for (const [key, text] of Object.entries(locales.en)) {
    const rules = textProblems(key, text);
    if (rules.length > 0) found[key] = { rules, reason: "" };
  }
  return found;
}

function disabledBreaks(): Allowed["disabled"] {
  const found: Allowed["disabled"] = {};
  for (const module of walk("src")) {
    const count = silentDisabled(readFileSync(join(ui, module), "utf8"));
    if (count > 0) found[module] = { count, reason: "" };
  }
  return found;
}

const BASELINE =
  "baseline 2026-10-11; the walk of this area fixes it (T-3848…T-3852)";

if (process.env.HUMAN_TEXT_RECORD) {
  const previous = json<Allowed>("tests/human_text.allow.json");
  const keep = <T extends { reason: string }>(
    now: Record<string, T>,
    before: Record<string, T>,
  ) =>
    Object.fromEntries(
      Object.entries(now)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, entry]) => [
          key,
          { ...entry, reason: before[key]?.reason || BASELINE },
        ]),
    );
  const recorded: Allowed = {
    $comment: previous.$comment,
    strings: keep(stringBreaks(), previous.strings),
    disabled: keep(disabledBreaks(), previous.disabled),
  };
  writeFileSync(
    join(ui, "tests/human_text.allow.json"),
    `${JSON.stringify(recorded, null, 2)}\n`,
  );
}

describe("the human text gate (T-3847)", () => {
  const allowed = json<Allowed>("tests/human_text.allow.json");

  it("has every key in every locale", () => {
    for (const locale of LOCALES.slice(1)) {
      expect(missingKeys(locales.en, locales[locale]), locale).toEqual([]);
    }
  });

  it("breaks no rule beyond the recorded list, and the list holds nothing already fixed", () => {
    const now = stringBreaks();
    const unrecorded = Object.entries(now)
      .filter(([key, { rules }]) =>
        rules.some((rule) => !allowed.strings[key]?.rules.includes(rule)),
      )
      .map(
        ([key, { rules }]) =>
          `${key} (${rules.join(", ")}): ${locales.en[key]}`,
      );
    expect(
      unrecorded,
      "write it as a person would read it (docs User-Guide/ui-writing.md)",
    ).toEqual([]);
    const fixed = Object.entries(allowed.strings)
      .filter(([key, { rules }]) =>
        rules.some((rule) => !now[key]?.rules.includes(rule)),
      )
      .map(([key]) => key);
    expect(
      fixed,
      "fixed: take these out of tests/human_text.allow.json",
    ).toEqual([]);
    for (const [key, { reason }] of Object.entries(allowed.strings)) {
      expect(reason.trim(), key).not.toBe("");
    }
  });

  it("disables no more controls without a reason than the recorded list", () => {
    const now = disabledBreaks();
    const worse = Object.entries(now)
      .filter(
        ([module, { count }]) => count > (allowed.disabled[module]?.count ?? 0),
      )
      .map(([module, { count }]) => `${module}: ${count}`);
    expect(
      worse,
      "give a disabled control a title or aria-describedby saying why",
    ).toEqual([]);
    const better = Object.entries(allowed.disabled)
      .filter(([module, { count }]) => (now[module]?.count ?? 0) < count)
      .map(([module]) => module);
    expect(
      better,
      "fewer now: lower these in tests/human_text.allow.json",
    ).toEqual([]);
  });

  it("records a walk entry for every module with controls, and enforces it once switched on", () => {
    const walked = json<Walk>("tests/human_ui_walk.json");
    const controls = Object.keys(
      json<{ modules: Record<string, unknown> }>("tests/controls.json").modules,
    );
    expect(
      controls.filter((module) => !(module in walked.modules)),
      "add these to tests/human_ui_walk.json",
    ).toEqual([]);
    if (walked.enforce) {
      expect(
        controls.filter((module) => walked.modules[module] === null),
        "not walked yet",
      ).toEqual([]);
    }
  });
});

describe("each rule fails its fixture and passes its fix", () => {
  it.each([
    ["generic label", "app.submit", "Submit", "generic", "Publish endpoint"],
    [
      "generic error",
      "app.failed",
      "Failed",
      "generic",
      "The endpoint was not saved. Try again in a minute.",
    ],
    ["urn", "entity.title", "urn:ngsi-ld:Vehicle:bus-12", "urn", "Bus 12"],
    [
      "uuid",
      "person.id",
      "3f2b1c4d-1a2b-4c3d-8e9f-0a1b2c3d4e5f",
      "uuid",
      "Jana Novak",
    ],
    [
      "kind name",
      "source.title",
      "Add an MqttBroker",
      "kind",
      "Add a message broker",
    ],
    [
      "http code",
      "endpoint.gone",
      "It answers 404 now.",
      "http",
      "It no longer answers.",
    ],
    [
      "json",
      "pipeline.example",
      'Send {"type": "Car"}',
      "json",
      "Send one car",
    ],
    [
      "no next step",
      "endpoints.saveFailed",
      "The endpoint could not be saved",
      "next-step",
      "The endpoint could not be saved. Try again.",
    ],
  ])("%s", (_, key, broken, rule, fixed) => {
    expect(textProblems(key, broken)).toContain(rule);
    expect(textProblems(key, fixed)).toEqual([]);
  });

  it("reads ICU arguments as the code's, not the reader's", () => {
    expect(textProblems("x.y", "Open {projectName}")).toEqual([]);
  });

  it("finds a key one locale lacks", () => {
    expect(missingKeys({ a: "A", b: "B" }, { a: "A" })).toEqual(["b"]);
    expect(missingKeys({ a: "A" }, { a: "A", c: "C" })).toEqual(["c"]);
  });

  it("counts a disabled control without a reason, and not one with", () => {
    expect(
      silentDisabled(
        "<Button disabled={busy} onClick={() => save()}>Save</Button>",
      ),
    ).toBe(1);
    expect(
      silentDisabled(
        '<Button disabled={busy} title={t("x.why")}>Save</Button>',
      ),
    ).toBe(0);
    expect(silentDisabled('<Input disabled aria-describedby="why" />')).toBe(0);
    expect(silentDisabled("<Badge disabled>x</Badge>")).toBe(0);
    expect(
      silentDisabled(
        "<Dialog footer={<><Button onClick={close}>Cancel</Button><Button disabled={bad}>Save</Button></>}>",
      ),
    ).toBe(1);
    expect(
      silentDisabled(
        '<Button disabled={n > 0} disabledReason={t("x.why")}>Go</Button>',
      ),
    ).toBe(0);
    expect(
      silentDisabled("<Button onClick={out} disabled={zoom <= min}>-</Button>"),
    ).toBe(1);
    expect(
      silentDisabled(
        "<Button onClick={() => go()} disabled={busy}>Go</Button>",
      ),
    ).toBe(1);
  });

  it("takes a second sentence or an instruction as the next step", () => {
    expect(hasNextStep("Nothing was saved. The server is busy.")).toBe(true);
    expect(hasNextStep("Could not save, try again")).toBe(true);
    expect(hasNextStep("Could not save")).toBe(false);
  });
});
