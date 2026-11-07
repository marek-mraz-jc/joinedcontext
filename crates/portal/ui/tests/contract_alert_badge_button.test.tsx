/**
 * T-1733, T-1734, T-1735: the UI contract for Alert, Badge and Button (UI-15, UI-16, UI-30).
 *
 * Each case names a defect these three shipped on 2026-09-20, found by reading the files against
 * the contract rather than by a failing test — which is why none of them had one.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Alert.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { Alert, Badge, Button } from "../src/components/ui";

const wrap = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (name: string) => readFileSync(join(ui, "src/components/ui", name), "utf8");

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("Alert", () => {
  it("an_error_is_announced_without_the_caller_remembering_to_ask", () => {
    // Seven of the eighty-two alerts in the Portal carried no role, three of them `danger`: an
    // error drawn in red and never spoken. The tone already says which it is.
    wrap(<Alert tone="danger">The endpoint could not be reached.</Alert>);
    expect(screen.getByRole("alert")).toHaveTextContent("The endpoint could not be reached.");

    wrap(<Alert tone="info">Six entities were imported.</Alert>);
    expect(screen.getByRole("status")).toHaveTextContent("Six entities were imported.");
  });

  it("a_caller_that_names_its_own_role_still_wins", () => {
    wrap(
      <Alert tone="danger" role="note">
        A refusal that is not news.
      </Alert>,
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("note")).toBeInTheDocument();
  });

  it("the_tone_reaches_a_screen_reader_as_a_word_not_as_a_colour", () => {
    // The icon is decorative and the box colour is invisible to a screen reader, so `warning` and
    // `success` were indistinguishable without the words.
    for (const [tone, word] of [
      ["danger", "Error"],
      ["warning", "Warning"],
      ["success", "Done"],
      ["info", "Note"],
    ] as const) {
      const { unmount } = wrap(<Alert tone={tone}>Something happened.</Alert>);
      expect(screen.getByText(word), tone).toBeInTheDocument();
      unmount();
    }
  });

  it("a_form_can_move_the_person_to_the_error_it_rendered", () => {
    const ref = createRef<HTMLDivElement>();
    wrap(
      <Alert ref={ref} tone="danger">
        Two fields need an answer.
      </Alert>,
    );
    expect(ref.current).toBeInstanceOf(HTMLDivElement);
  });

  it("what_it_is_given_is_text_never_markup", () => {
    wrap(<Alert tone="danger">{'<img src=x onerror="alert(1)">'}</Alert>);
    expect(screen.getByRole("alert").querySelector("img")).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent('<img src=x onerror="alert(1)">');
  });
});

describe("Badge", () => {
  it("every_tone_is_a_token_so_the_theme_and_the_brand_reach_all_of_them", () => {
    // `purple` was Tailwind's own `purple-500`, which no `@theme` entry defines: the one tone
    // that stayed the same colour in dark mode and under an installation's brand.
    const source = read("Badge.tsx");
    const tones = /const TONES[^=]*= \{([\s\S]*?)\n\};/.exec(source)?.[1] ?? "";
    expect(tones).not.toBe("");
    expect(
      tones.match(
        /\b(?:bg|text|border)-(?:red|blue|green|gray|slate|zinc|amber|yellow|emerald|sky|indigo|rose|orange|purple|violet|fuchsia|pink|teal|cyan|lime|stone|neutral)-\d{2,3}\b/g,
      ),
    ).toBeNull();
  });

  it("a_long_label_wraps_instead_of_running_past_its_column", () => {
    // A chip carries a translated label into a table cell: "natives Bloblang, ungeprüft".
    wrap(<Badge>natives Bloblang, ungeprüft</Badge>);
    expect(screen.getByText("natives Bloblang, ungeprüft").className).not.toMatch(
      /whitespace-nowrap/,
    );
  });

  it("mono_and_className_both_reach_the_chip_and_a_ref_does_too", () => {
    const ref = createRef<HTMLSpanElement>();
    wrap(
      <Badge ref={ref} mono className="mt-3" data-testid="chip">
        ep-bikes
      </Badge>,
    );
    const chip = screen.getByTestId("chip");
    expect(chip.className).toMatch(/font-mono/);
    expect(chip.className).toMatch(/mt-3/);
    expect(ref.current).toBe(chip);
  });
});

describe("Button", () => {
  it("a_button_a_person_may_not_use_says_why_and_can_still_be_reached", async () => {
    // UI-44. `disabled` takes the button out of the tab order, and
    // `disabled:pointer-events-none` suppresses the tooltip: two call sites wrote a reason the
    // browser then threw away, so nobody was ever told.
    const onClick = vi.fn();
    wrap(
      <Button disabled disabledReason="This endpoint is not live yet." onClick={onClick}>
        Open the data
      </Button>,
    );
    const button = screen.getByRole("button", { name: /Open the data/ });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("This endpoint is not live yet.");
    expect(button).toHaveAttribute("title", "This endpoint is not live yet.");

    await userEvent.click(button);
    expect(onClick, "an explained button still refuses the click").not.toHaveBeenCalled();

    button.focus();
    expect(button, "and it stays reachable, or the reason is never read").toHaveFocus();
  });

  it("a_caller_s_own_description_and_title_keep_the_reason", () => {
    // F-UI1-014: a caller's `aria-describedby` or `title` replaced the reason's link and tooltip.
    wrap(
      <>
        <p id="publish-hint">Publishing makes the app public.</p>
        <Button disabled disabledReason="Check the app first." aria-describedby="publish-hint" title="Publish">
          Publish
        </Button>
      </>,
    );
    const button = screen.getByRole("button", { name: /Publish/ });
    expect(button).toHaveAccessibleDescription("Check the app first. Publishing makes the app public.");
    expect(button).toHaveAttribute("title", "Check the app first.");
  });

  it("a_reason_on_its_own_changes_nothing_because_disabled_is_what_refuses", async () => {
    // The pair is `disabled` beside the reason, and this case pins why it may not be loosened:
    // a call site may hand one constant reason to a list and compute `disabled` per row
    // (`AppGenerator`'s endpoint boxes) or per state (`SaveAsDialog`'s "check first"). Making a
    // reason refuse on its own turns those two into controls nobody can use (T-2428). What the
    // reason must not do is claim a refusal it does not make: with no `disabled` it is neither
    // shown as a tooltip nor announced.
    const onClick = vi.fn();
    wrap(
      <Button disabledReason="Only when three are chosen." onClick={onClick}>
        Add another
      </Button>,
    );
    const button = screen.getByRole("button", { name: "Add another" });
    expect(button).toBeEnabled();
    expect(button).not.toHaveAttribute("aria-disabled");
    expect(button).not.toHaveAttribute("title");
    expect(button).not.toHaveAccessibleDescription();

    await userEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("a_button_disabled_without_a_reason_behaves_as_it_always_did", () => {
    wrap(<Button disabled>Save</Button>);
    const button = screen.getByRole("button", { name: "Save" });
    expect(button).toBeDisabled();
    expect(button).not.toHaveAttribute("aria-disabled");
  });

  it("the_spinner_stops_for_somebody_who_asked_motion_to_stop", () => {
    wrap(<Button loading>Publishing</Button>);
    const spinner = screen.getByRole("button", { name: /Publishing/ }).querySelector("svg");
    expect(spinner?.getAttribute("class")).toMatch(/motion-safe:animate-spin/);
    expect(spinner?.getAttribute("class")).not.toMatch(/(?<!motion-safe:)animate-spin/);
  });

  it("a_long_label_wraps_instead_of_running_past_its_container", () => {
    // "Eine Änderung mit dem Assistenten vorschlagen" is 45 characters against 35 in English.
    wrap(<Button>Eine Änderung mit dem Assistenten vorschlagen</Button>);
    expect(screen.getByRole("button").className).not.toMatch(/whitespace-nowrap/);
  });

  it("pressing_a_ghost_button_looks_different_from_hovering_it", () => {
    const source = read("Button.tsx");
    const variants = /const VARIANTS[^=]*= \{([\s\S]*?)\n\};/.exec(source)?.[1] ?? "";
    expect(variants).not.toBe("");
    for (const line of variants.split("\n").filter((row) => row.includes(":"))) {
      const hover = /hover:(bg-[\w-]+)/.exec(line)?.[1];
      const active = /active:(bg-[\w-]+)/.exec(line)?.[1];
      if (hover && active) expect(active, line.trim()).not.toBe(hover);
    }
    // And no variant reaches for opacity instead of a colour: fading the button fades its label
    // with it, and `transition-colors` never covered opacity.
    expect(variants).not.toMatch(/(?:hover|active):opacity-/);
  });
});

/**
 * T-2409: a refused submit does not submit.
 *
 * `disabledReason` makes the button `aria-disabled` rather than `disabled`, so it keeps its place
 * in the tab order and can say why it is closed. `aria-disabled` means nothing to the browser
 * though: a `type="submit"` button still submits the form it stands in, and dropping the React
 * `onClick` does not stop that. Every schema-driven form of the Portal closes its submit through
 * this prop, so "proposing is closed" and "your role may not propose a ContextSpace" sent the
 * proposal all the same.
 */
describe("a refused button refuses the whole click", () => {
  it("a_refused_submit_does_not_submit_the_form_it_stands_in", async () => {
    const onSubmit = vi.fn((event: React.FormEvent) => event.preventDefault());
    render(
      <I18nextProvider i18n={i18n}>
        <form onSubmit={onSubmit}>
          <Button type="submit" disabled disabledReason="Proposing is closed.">
            Propose
          </Button>
        </form>
      </I18nextProvider>,
    );
    const button = screen.getByRole("button", { name: "Propose" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(button);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("a_refused_submit_reached_by_the_keyboard_does_not_submit_either", async () => {
    const onSubmit = vi.fn((event: React.FormEvent) => event.preventDefault());
    render(
      <I18nextProvider i18n={i18n}>
        <form onSubmit={onSubmit}>
          <Button type="submit" disabled disabledReason="Proposing is closed.">
            Propose
          </Button>
        </form>
      </I18nextProvider>,
    );
    screen.getByRole("button", { name: "Propose" }).focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard(" ");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("a_submit_nobody_refused_still_submits", async () => {
    const onSubmit = vi.fn((event: React.FormEvent) => event.preventDefault());
    render(
      <I18nextProvider i18n={i18n}>
        <form onSubmit={onSubmit}>
          <Button type="submit">Propose</Button>
        </form>
      </I18nextProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Propose" }));
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });
});
