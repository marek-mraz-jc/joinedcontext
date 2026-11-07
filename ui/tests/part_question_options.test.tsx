/**
 * T-1839: the assistant's question, against the UI contract (UI-15, UI-16, UI-11, UI-44).
 *
 * `question_options.test.tsx` owns what the part does — one click answers, several toggle,
 * eight become a searchable list, the arrows move between them. This file owns the contract
 * around that behaviour, and the three things the survey of 2026-09-18 asked about:
 *
 * - the choice chip stays hand-made, so its keyboard contract is held here rather than by the
 *   shared `Button`: exactly one chip in the tab order, the arrows moving it, the group named;
 * - the press that gathers several answers is refused with a reason the screen reader gets
 *   (UI-44), instead of being hard-disabled with the reason written in a span beside it;
 * - every string of the part is translated, in all four locales, and every title and
 *   description the assistant sends is drawn as text.
 */
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { JsonSchema } from "../src/components/forms/types";
import { QuestionOptions } from "../src/pages/apps/QuestionOptions";
import { expectDenied, expectNoRawKeys, focusables } from "./checks";
import { expectNoAxeViolations, inEveryLocale, renderPart } from "./page_contract";

const BIKES = { const: "bikes", title: "City bikes", description: "space helsinki · ngsi-ld" };
const AIR = { const: "air", title: "Air quality", description: "space helsinki · csv" };

/** A question with one answer, drawn as a row of chips. */
function one(options: { const: string; title: string; description?: string }[], extra = {}): JsonSchema {
  return {
    type: "object",
    title: "Which endpoint?",
    properties: { answer: { type: "string", title: "Which endpoint?", oneOf: options, ...extra } },
    required: ["answer"],
  } as JsonSchema;
}

/** A question with several answers, drawn as toggles and one "Use these" press. */
function several(
  options: { const: string; title: string; description?: string }[],
  extra: Record<string, unknown> = {},
): JsonSchema {
  return {
    type: "object",
    title: "Which endpoints should the app read?",
    properties: {
      answer: {
        type: "array",
        title: "Which endpoints should the app read?",
        items: { type: "string", oneOf: options },
        uniqueItems: true,
        ...extra,
      },
    },
    required: ["answer"],
  } as JsonSchema;
}

const MANY = Array.from({ length: 8 }, (_, at) => ({
  const: `e${at}`,
  title: `Endpoint ${at}`,
  description: at === 5 ? "space espoo" : "space helsinki",
}));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("the chips of a question, as a keyboard reaches them", () => {
  // UI-15. The chip is the one control of this part the shared Button cannot be — it is a
  // radio with a title over a description — so the roving tab order it implements by hand is
  // what this asserts: Tab reaches the group once, and the arrows do the rest.
  it("puts one chip in the tab order and moves it with the arrows", async () => {
    const user = userEvent.setup();
    const { container } = renderPart(<QuestionOptions schema={one([BIKES, AIR])} onAnswer={vi.fn()} />);

    const chips = screen.getAllByRole("radio");
    // Tab stops at the group once: every chip but one is taken out of the tab order by hand,
    // which is the contract a roving-tabindex control owes (UI-15).
    expect(chips.filter((chip) => chip.tabIndex === 0)).toEqual([chips[0]]);
    expect(focusables(container)).toContain(chips[0]);

    chips[0].focus();
    await user.keyboard("{ArrowRight}");
    expect(chips[1]).toHaveFocus();
    expect(chips[1]).toHaveAttribute("tabindex", "0");
    expect(chips[0]).toHaveAttribute("tabindex", "-1");
    // The ends wrap, so nothing is unreachable from where the focus happens to be.
    await user.keyboard("{ArrowRight}");
    expect(chips[0]).toHaveFocus();
  });

  it("names the group of chips with the question, for one answer and for several", () => {
    renderPart(<QuestionOptions schema={one([BIKES, AIR])} onAnswer={vi.fn()} />);
    const group = screen.getByRole("radiogroup", { name: "Which endpoint?" });
    expect(within(group).getAllByRole("radio")).toHaveLength(2);

    cleanup();
    renderPart(<QuestionOptions schema={several([BIKES, AIR])} onAnswer={vi.fn()} />);
    expect(
      screen.getByRole("group", { name: "Which endpoints should the app read?" }),
    ).toBeInTheDocument();
  });

  // The search box filters the chips, so the roving index has to follow the list that is left:
  // pointing past it would leave no chip in the tab order at all.
  it("keeps a chip in the tab order after the search has narrowed the list", async () => {
    const user = userEvent.setup();
    renderPart(<QuestionOptions schema={one(MANY)} onAnswer={vi.fn()} />);

    const search = screen.getByRole("searchbox", { name: en.agentRun.question.search });
    search.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getAllByRole("radio")[0]).not.toHaveFocus();

    await user.type(search, "espoo");
    const left = screen.getAllByRole("radio");
    expect(left).toHaveLength(1);
    expect(left[0]).toHaveAttribute("tabindex", "0");
  });

  it("says so when the search matches nothing, instead of showing an empty box", async () => {
    const user = userEvent.setup();
    renderPart(<QuestionOptions schema={one(MANY)} onAnswer={vi.fn()} />);

    await user.type(screen.getByRole("searchbox", { name: en.agentRun.question.search }), "tampere");
    expect(screen.queryAllByRole("radio")).toEqual([]);
    expect(screen.getByText(en.agentRun.question.noMatch)).toBeInTheDocument();
  });
});

describe("the press that sends several answers", () => {
  // UI-44, T-1743. Too few chosen used to hard-disable the press, which took it out of the tab
  // order and left "Choose at least 1." in a span beside it that nothing pointed at: someone
  // who could not see the button greyed out could neither reach it nor be told why.
  it("is refused with its reason while too few are chosen, and stays reachable", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    const { container } = renderPart(
      <QuestionOptions schema={several([BIKES, AIR], { minItems: 1 })} onAnswer={onAnswer} />,
    );

    const use = screen.getByRole("button", { name: "Use these (0)" });
    expectDenied(use, "Choose at least 1.");
    expect(focusables(container)).toContain(use);
    // The sentence is on the screen as well, for whoever reads the page — and hidden from the
    // screen reader there, so the refusal is not announced twice.
    const hint = screen.getByText("Choose at least 1.", { ignore: ".sr-only" });
    expect(hint).toHaveAttribute("aria-hidden", "true");
    await user.click(use);
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("is refused with its reason once one too many is chosen", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    renderPart(
      <QuestionOptions schema={several([BIKES, AIR], { maxItems: 1 })} onAnswer={onAnswer} />,
    );

    await user.click(screen.getByRole("button", { name: /City bikes/ }));
    await user.click(screen.getByRole("button", { name: /Air quality/ }));
    const use = screen.getByRole("button", { name: "Use these (2)" });
    expectDenied(use, "Choose at most 1.");
    await user.click(use);
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("takes the press once the count is within the range", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    renderPart(
      <QuestionOptions schema={several([BIKES, AIR], { minItems: 1, maxItems: 2 })} onAnswer={onAnswer} />,
    );

    await user.click(screen.getByRole("button", { name: /City bikes/ }));
    const use = screen.getByRole("button", { name: "Use these (1)" });
    expect(use).not.toHaveAttribute("aria-disabled");
    await user.click(use);
    expect(onAnswer).toHaveBeenCalledWith({ answer: ["bikes"] });
  });

  // While an answer is on its way there is no reason to read and nothing to reach: the whole
  // question is out for the moment it takes, which is a wait rather than a refusal.
  it("is hard-disabled, with no reason, while an answer is on its way", () => {
    renderPart(
      <QuestionOptions schema={several([BIKES, AIR], { minItems: 1 })} onAnswer={vi.fn()} disabled />,
    );

    for (const button of screen.getAllByRole("button")) {
      expect(button).toBeDisabled();
      expect(button).not.toHaveAttribute("aria-disabled");
    }
  });
});

describe("what the assistant sends is data", () => {
  // AG-46. The titles and descriptions come from the run's stream, so a question that carries
  // markup is drawn as the characters it is, not parsed into an element.
  it("draws a title and a description that arrive as markup as text", () => {
    const { container } = renderPart(
      <QuestionOptions
        schema={one([
          { const: "x", title: "<img src=x onerror=alert(1)>", description: "<b>space</b> helsinki" },
        ])}
        onAnswer={vi.fn()}
      />,
    );

    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(screen.getByText("<b>space</b> helsinki")).toBeInTheDocument();
  });

  it("answers with the value of the choice, never with its title", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    renderPart(<QuestionOptions schema={one([BIKES, AIR])} onAnswer={onAnswer} />);

    await user.click(screen.getByRole("radio", { name: /City bikes/ }));
    expect(onAnswer).toHaveBeenCalledWith({ answer: "bikes" });
  });
});

describe("the question in every locale", () => {
  // UI-11. The chips carry the assistant's own words, so what is translated is everything
  // around them: the search box, the range, the press and the "something else" way out.
  it("translates every string it owns, in all four locales", async () => {
    await inEveryLocale(async (locale) => {
      const { container, unmount } = renderPart(
        <QuestionOptions schema={several(MANY, { minItems: 2 })} onAnswer={vi.fn()} />,
      );
      expectNoRawKeys(container);
      expect(
        screen.getByRole("searchbox"),
        `${locale} has no search box`,
      ).toBeInTheDocument();
      // Nothing of the part's own text is left in English once the language is not English.
      if (locale !== "en") {
        expect(screen.queryByText(en.agentRun.conversation.somethingElse)).toBeNull();
      }
      unmount();
    });
  });

  it("has no axe violations as a searchable list, nor as a row of toggles", async () => {
    const list = renderPart(<QuestionOptions schema={one(MANY)} onAnswer={vi.fn()} />);
    await expectNoAxeViolations(list.container);
    list.unmount();

    const toggles = renderPart(
      <QuestionOptions schema={several([BIKES, AIR], { minItems: 1 })} onAnswer={vi.fn()} />,
    );
    await expectNoAxeViolations(toggles.container);
  });
});
