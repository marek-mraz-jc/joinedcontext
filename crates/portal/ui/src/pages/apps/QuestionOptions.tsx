import type { JSX, KeyboardEvent } from "react";
import { Fragment, Suspense, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { SchemaForm } from "../../components/forms/LazySchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { Button, Input, PageLoading } from "../../components/ui";

/** One option of a question: the value the answer carries and what the person reads. */
export interface Choice {
  value: string;
  title: string;
  description?: string;
}

/** A question whose answer is one or several of a list (UI-57, UI-73). */
export interface Choices {
  question: string;
  multiple: boolean;
  choices: Choice[];
  min?: number;
  max?: number;
  suggested: string[];
}

/** Above this many, the options are a searchable list, never a row cut short (UI-74). */
const ROW = 6;

function choiceOf(one: unknown): Choice | null {
  if (typeof one === "string") {
    return { value: one, title: one };
  }
  const option = one as { const?: unknown; title?: unknown; description?: unknown } | null;
  if (typeof option?.const !== "string") {
    return null;
  }
  return {
    value: option.const,
    title: typeof option.title === "string" && option.title !== "" ? option.title : option.const,
    description: typeof option.description === "string" ? option.description : undefined,
  };
}

function listOf(answer: { enum?: unknown; oneOf?: unknown }): Choice[] {
  const raw = Array.isArray(answer.enum) ? answer.enum : Array.isArray(answer.oneOf) ? answer.oneOf : [];
  return raw.map(choiceOf).filter((choice): choice is Choice => choice !== null);
}

/** The options of a question whose answer is one or several of a list, or `null` for free text. */
export function choicesOf(schema: JsonSchema | undefined): Choices | null {
  const answer = (schema as { properties?: Record<string, unknown> } | undefined)?.properties?.answer as
    | {
        type?: unknown;
        title?: unknown;
        enum?: unknown;
        oneOf?: unknown;
        items?: { enum?: unknown; oneOf?: unknown };
        minItems?: unknown;
        maxItems?: unknown;
        default?: unknown;
      }
    | undefined;
  if (!answer) {
    return null;
  }
  const multiple = answer.type === "array";
  const choices = listOf(multiple ? (answer.items ?? {}) : answer);
  if (choices.length === 0) {
    return null;
  }
  const given = Array.isArray(answer.default) ? answer.default : [answer.default];
  const values = new Set(choices.map((choice) => choice.value));
  return {
    question:
      typeof answer.title === "string"
        ? answer.title
        : typeof (schema as { title?: unknown }).title === "string"
          ? String((schema as { title?: unknown }).title)
          : "",
    multiple,
    choices,
    min: typeof answer.minItems === "number" ? answer.minItems : undefined,
    max: typeof answer.maxItems === "number" ? answer.maxItems : undefined,
    suggested: given.filter((value): value is string => typeof value === "string" && values.has(value)),
  };
}

/** The values of a list question, or `null` when it is free text. */
export function optionsOf(schema: JsonSchema | undefined): string[] | null {
  return choicesOf(schema)?.choices.map((choice) => choice.value) ?? null;
}

/**
 * One question of the assistant (UI-57, UI-73, UI-74). One answer is a row of option cards, one
 * click answers; several answers are the same cards as toggles and a "Use these" press; more than
 * six are a list with a search box. "Something else…" opens the form, only where free text is an
 * answer: never for a question the platform filled (`free` false), whose answers it checks.
 */
export function QuestionOptions({
  schema,
  disabled,
  free = true,
  disabledReasons = {},
  onAnswer,
}: {
  schema: JsonSchema;
  disabled?: boolean;
  free?: boolean;
  /** Options the person cannot take now, each with why (UI-44, T-2694): shown, never taken. */
  disabledReasons?: Record<string, string>;
  onAnswer: (data: unknown) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const labelId = useId();
  const [typing, setTyping] = useState(false);
  const found = choicesOf(schema);
  const [chosen, setChosen] = useState<string[]>(found?.multiple ? found.suggested : []);
  const [search, setSearch] = useState("");
  const [focus, setFocus] = useState(0);
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);

  if (!found || typing) {
    return (
      <Suspense fallback={<PageLoading label={t("app.loading")} lines={1} />}>
        <SchemaForm
          schema={schema}
          disabled={disabled}
          submitLabel={t("agentRun.conversation.answer")}
          onSubmit={onAnswer}
        />
      </Suspense>
    );
  }

  const needle = search.trim().toLowerCase();
  const shown = found.choices.filter(
    (choice) =>
      needle === "" ||
      choice.title.toLowerCase().includes(needle) ||
      (choice.description ?? "").toLowerCase().includes(needle),
  );
  const min = found.min ?? 0;
  const max = found.max ?? found.choices.length;
  const within = chosen.length >= min && chosen.length <= max;

  const toggle = (value: string): void => {
    setChosen((now) => (now.includes(value) ? now.filter((one) => one !== value) : [...now, value]));
  };
  const pick = (value: string): void => {
    if (disabledReasons[value] !== undefined) {
      return;
    }
    if (found.multiple) {
      toggle(value);
    } else {
      onAnswer({ answer: value });
    }
  };
  // Roving focus: one option is in the tab order, the arrows move between them (UI-74).
  const onKey = (event: KeyboardEvent<HTMLButtonElement>, at: number): void => {
    const step = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : event.key === "ArrowUp" || event.key === "ArrowLeft" ? -1 : 0;
    if (step !== 0) {
      event.preventDefault();
      const next = (at + step + shown.length) % shown.length;
      setFocus(next);
      buttons.current[next]?.focus();
    } else if (event.key === "Enter" && found.multiple) {
      event.preventDefault();
      if (within) {
        onAnswer({ answer: chosen });
      }
    }
  };
  const range =
    found.min !== undefined && found.max !== undefined
      ? t("agentRun.question.between", { min: found.min, max: found.max })
      : found.min !== undefined
        ? t("agentRun.question.atLeast", { min: found.min })
        : found.max !== undefined
          ? t("agentRun.question.atMost", { max: found.max })
          : null;

  return (
    <div className="space-y-2" data-testid="question-options">
      {found.question ? (
        <p id={labelId} className="text-body">
          {found.question}
        </p>
      ) : null}
      {found.choices.length > ROW ? (
        <Input
          type="search"
          aria-label={t("agentRun.question.search")}
          placeholder={t("agentRun.question.search")}
          value={search}
          disabled={disabled}
          title={disabled ? t("app.busyReason") : undefined}
          onChange={(event) => {
            setSearch(event.target.value);
            setFocus(0);
          }}
        />
      ) : null}
      <div
        role={found.multiple ? "group" : "radiogroup"}
        aria-labelledby={found.question ? labelId : undefined}
        className={found.choices.length > ROW ? "max-h-72 space-y-1 overflow-y-auto" : "flex flex-wrap gap-2"}
      >
        {shown.map((choice, at) => {
          const on = chosen.includes(choice.value);
          const reason = disabledReasons[choice.value];
          const reasonId = `${labelId}-reason-${at}`;
          return (
            <Fragment key={choice.value}>
              <button
                ref={(element) => {
                  buttons.current[at] = element;
                }}
                type="button"
                role={found.multiple ? undefined : "radio"}
                aria-checked={found.multiple ? undefined : false}
                aria-pressed={found.multiple ? on : undefined}
                tabIndex={at === Math.min(focus, shown.length - 1) ? 0 : -1}
                disabled={disabled}
                // Refused, not removed: it keeps its place in the arrows' order and says why (UI-44).
                aria-disabled={reason !== undefined || undefined}
                aria-describedby={reason !== undefined ? reasonId : undefined}
                onClick={() => pick(choice.value)}
                onKeyDown={(event) => onKey(event, at)}
                className={`rounded-md border px-3 py-2 text-left text-body disabled:opacity-50 aria-disabled:cursor-not-allowed aria-disabled:opacity-60 ${
                  on ? "border-primary bg-primary/10" : "border-border bg-surface hover:bg-surface-subtle"
                } ${found.choices.length > ROW ? "block w-full" : ""}`}
              >
                <span className="font-medium">{choice.title}</span>
                {!found.multiple && found.suggested.includes(choice.value) ? (
                  <span className="ml-2 text-caption text-fg-muted">{t("agentRun.question.suggested")}</span>
                ) : null}
                {choice.description ? (
                  <span className="block text-caption text-fg-muted">{choice.description}</span>
                ) : null}
                {/* Seen here; heard once, as the description beside the button, not in its name. */}
                {reason !== undefined ? (
                  <span aria-hidden="true" className="block text-caption text-fg-muted">
                    {reason}
                  </span>
                ) : null}
              </button>
              {reason !== undefined ? (
                <span id={reasonId} className="sr-only">
                  {reason}
                </span>
              ) : null}
            </Fragment>
          );
        })}
        {shown.length === 0 ? <p className="text-body text-fg-muted">{t("agentRun.question.noMatch")}</p> : null}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {found.multiple ? (
          <>
            <Button
              size="sm"
              disabled={disabled || !within}
              // Too few or too many chosen is a refusal with a reason, so the press keeps its
              // place in the tab order and says why it will not be taken (UI-44, T-1743): the
              // range was written beside the button, where a screen reader never joined the two.
              // While an answer is on its way there is no reason to read, and the whole question
              // is hard-disabled for the moment it takes.
              disabledReason={!disabled && !within ? (range ?? undefined) : undefined}
              onClick={() => onAnswer({ answer: chosen })}
            >
              {t("agentRun.question.useThese", { count: chosen.length })}
            </Button>
            {/* The same sentence the refused press carries as its reason, kept on screen for
                whoever reads the page rather than hears it, and hidden from the screen reader so
                it is not read twice in a row. */}
            {range ? (
              <span aria-hidden="true" className="text-caption text-fg-muted">
                {range}
              </span>
            ) : null}
          </>
        ) : null}
        {free && !found.multiple ? (
          <Button size="sm" variant="ghost" disabled={disabled} disabledReason={t("app.busyReason")} onClick={() => setTyping(true)}>
            {t("agentRun.conversation.somethingElse")}
          </Button>
        ) : null}
      </div>
    </div>
  );
}
