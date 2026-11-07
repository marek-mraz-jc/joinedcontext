import { render, screen, within } from "@testing-library/react";
import axe from "axe-core";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { optionsOf, QuestionOptions } from "../src/pages/apps/QuestionOptions";
import { expectDenied } from "./checks";
import type { JsonSchema } from "../src/components/forms/types";

const ENUM: JsonSchema = {
  type: "object",
  title: "Which context space?",
  properties: {
    answer: { type: "string", title: "Which context space?", enum: ["helsinki", "helsinki-kpi"] },
  },
  required: ["answer"],
} as JsonSchema;

const TEXT: JsonSchema = {
  type: "object",
  title: "What should it be called?",
  properties: { answer: { type: "string", title: "What should it be called?" } },
  required: ["answer"],
} as JsonSchema;

function show(schema: JsonSchema, onAnswer = vi.fn(), props: { free?: boolean; disabled?: boolean } = {}) {
  render(
    <I18nextProvider i18n={i18n}>
      <QuestionOptions schema={schema} onAnswer={onAnswer} {...props} />
    </I18nextProvider>,
  );
  return onAnswer;
}

function one(options: { const: string; title: string; description?: string }[], extra = {}): JsonSchema {
  return {
    type: "object",
    title: "Which endpoint?",
    properties: { answer: { type: "string", title: "Which endpoint?", oneOf: options, ...extra } },
    required: ["answer"],
  } as JsonSchema;
}

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

const BIKES = { const: "bikes", title: "City bikes", description: "space helsinki · ngsi-ld, geojson" };
const AIR = { const: "air", title: "Air quality", description: "space helsinki · csv" };

describe("a question of the assistant (UI-57)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("draws one button per option and answers with one click", async () => {
    const onAnswer = show(ENUM);

    expect(screen.getByRole("radio", { name: "helsinki-kpi" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: "helsinki" }));
    expect(onAnswer).toHaveBeenCalledWith({ answer: "helsinki" });
  });

  it("opens the text box for the answer nobody listed", async () => {
    show(ENUM);

    await userEvent.click(
      screen.getByRole("button", { name: en.agentRun.conversation.somethingElse }),
    );
    expect(screen.queryByTestId("question-options")).toBeNull();
    expect(screen.getByRole("button", { name: en.agentRun.conversation.answer })).toBeInTheDocument();
  });

  it("keeps the form for a question that is not a list", () => {
    show(TEXT);

    expect(screen.queryByTestId("question-options")).toBeNull();
    expect(screen.getByRole("button", { name: en.agentRun.conversation.answer })).toBeInTheDocument();
  });

  it("reads the options of an enum and of a oneOf of consts, and nothing else", () => {
    expect(optionsOf(ENUM)).toEqual(["helsinki", "helsinki-kpi"]);
    expect(
      optionsOf({
        type: "object",
        properties: { answer: { oneOf: [{ const: "yes" }, { const: "no" }] } },
      } as JsonSchema),
    ).toEqual(["yes", "no"]);
    expect(optionsOf(TEXT)).toBeNull();
    expect(optionsOf({ type: "object", properties: {} } as JsonSchema)).toBeNull();
  });

  it("shows a oneOf's titles and descriptions and answers with the const", async () => {
    const onAnswer = show(one([BIKES, AIR], { default: "air" }));

    expect(screen.getByText("space helsinki · csv")).toBeInTheDocument();
    const air = screen.getByRole("radio", { name: /Air quality/ });
    expect(within(air).getByText(en.agentRun.question.suggested)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: /City bikes/ }));
    expect(onAnswer).toHaveBeenCalledWith({ answer: "bikes" });
  });

  it("toggles several answers and sends them on one press", async () => {
    const onAnswer = show(several([BIKES, AIR, { const: "noise", title: "Noise" }], { minItems: 1 }));

    const use = screen.getByRole("button", { name: "Use these (0)" });
    // Refused, not hard-disabled: the range is the button's own reason now, so it reaches
    // somebody who cannot see it greyed out (UI-44, T-1743, T-1839).
    expectDenied(use, "Choose at least 1.");
    await userEvent.click(screen.getByRole("button", { name: /City bikes/ }));
    await userEvent.click(screen.getByRole("button", { name: /Air quality/ }));
    expect(screen.getByRole("button", { name: /City bikes/ })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "Use these (2)" }));
    expect(onAnswer).toHaveBeenCalledWith({ answer: ["bikes", "air"] });
  });

  it("starts several answers from the suggested set and keeps them within max", async () => {
    const onAnswer = show(several([BIKES, AIR], { maxItems: 1, default: ["air"] }));

    expect(screen.getByRole("button", { name: /Air quality/ })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: /City bikes/ }));
    expectDenied(screen.getByRole("button", { name: "Use these (2)" }), "Choose at most 1.");
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("draws eight options as a list with a search over title and description", async () => {
    const eight = Array.from({ length: 8 }, (_, at) => ({
      const: `e${at}`,
      title: `Endpoint ${at}`,
      description: at === 5 ? "space espoo" : "space helsinki",
    }));
    show(one(eight));

    expect(screen.getAllByRole("radio")).toHaveLength(8);
    await userEvent.type(screen.getByRole("searchbox", { name: en.agentRun.question.search }), "espoo");
    expect(screen.getAllByRole("radio")).toHaveLength(1);
    expect(screen.getByRole("radio", { name: /Endpoint 5/ })).toBeInTheDocument();
  });

  it("answers by keyboard alone: arrows move, Space toggles, Enter answers", async () => {
    const onAnswer = show(several([BIKES, AIR], { minItems: 1 }));

    const bikes = screen.getByRole("button", { name: /City bikes/ });
    bikes.focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("button", { name: /Air quality/ })).toHaveFocus();
    await userEvent.keyboard(" ");
    await userEvent.keyboard("{Enter}");
    expect(onAnswer).toHaveBeenCalledWith({ answer: ["air"] });
  });

  it("labels the group with its question", () => {
    show(several([BIKES, AIR]));

    expect(screen.getByRole("group", { name: "Which endpoints should the app read?" })).toBeInTheDocument();
  });

  it("offers no free text for a question the platform filled", () => {
    show(one([BIKES, AIR]), vi.fn(), { free: false });

    expect(screen.queryByRole("button", { name: en.agentRun.conversation.somethingElse })).toBeNull();
  });

  it("disables every option while an answer is on its way", () => {
    show(several([BIKES, AIR]), vi.fn(), { disabled: true });

    for (const button of screen.getAllByRole("button")) {
      expect(button).toBeDisabled();
    }
  });

  it("has no axe violations", async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <main>
          <QuestionOptions schema={several([BIKES, AIR], { minItems: 1 })} onAnswer={vi.fn()} />
        </main>
      </I18nextProvider>,
    );
    const results = await axe.run(document.body);
    expect(results.violations.map((v) => v.id)).toEqual([]);
  });
});
