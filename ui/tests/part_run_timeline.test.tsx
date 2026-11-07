/**
 * T-1841: the run timeline against the UI contract (UI-11, UI-16, AG-43…AG-46).
 *
 * No test named this file. It is the eight states of Architecture/19 §5 drawn as a list rather
 * than a spinner, so what it owns is: exactly one step marked current, a run that stopped
 * saying so in words and not in colour alone, the expired run kept apart from the failed one
 * (T-0669), every state named in all four languages, and axe clean in each.
 */
import { cleanup, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RunTimeline } from "../src/pages/apps/RunTimeline";
import { RUN_STATES } from "../src/pages/apps/useAgentRun";
import { expectNoAxeViolations, inEveryLocale, renderPart } from "./page_contract";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("the run timeline", () => {
  it("names itself and lists every state of a run", async () => {
    renderPart(<RunTimeline status="building" steps={12} tokensUsed={48210} />);
    expect(
      screen.getByRole("region", { name: en.agentRun.timeline.title }),
    ).toBeInTheDocument();
    const steps = screen.getAllByRole("listitem");
    expect(steps).toHaveLength(RUN_STATES.length);
    expect(steps[0]).toHaveTextContent(en.agentRun.states.queued);
  });

  // One current step, and it is marked as a step rather than merely coloured: a screen reader
  // is told where the run stands, which the colour alone never says (UI-16).
  it("marks exactly one state as the one the run is in", () => {
    renderPart(<RunTimeline status="testing" steps={3} tokensUsed={10} />);
    const current = screen
      .getAllByRole("listitem")
      .filter((item) => item.getAttribute("aria-current") === "step");
    expect(current).toHaveLength(1);
    expect(current[0]).toHaveTextContent(en.agentRun.states.testing);
  });

  it("marks nothing as current for a state that is not on the timeline", () => {
    renderPart(<RunTimeline status="not-a-state" steps={0} tokensUsed={0} />);
    expect(
      screen.getAllByRole("listitem").filter((item) => item.hasAttribute("aria-current")),
    ).toHaveLength(0);
  });

  // A run that stopped says so in words. `failed` and `cancelled` are failures; `expired` ended
  // with its preview intact and is not drawn as one (T-0669).
  it.each(["failed", "cancelled", "expired"] as const)("says a %s run has stopped", (status) => {
    renderPart(<RunTimeline status={status} steps={7} tokensUsed={99} />);
    const said = screen.getAllByRole("status");
    expect(said.some((node) => node.textContent === en.agentRun.states[status])).toBe(true);
  });

  it("does not say a published run stopped", () => {
    renderPart(<RunTimeline status="published" steps={7} tokensUsed={99} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  // What the run has spent, with the thousands separated in the Portal's language, never the
  // browser's (T-3278): 48,210 in English, 48 210 in Slovak.
  it("counts the steps and the tokens", () => {
    renderPart(<RunTimeline status="building" steps={12} tokensUsed={48210} />);
    expect(
      screen.getByText(
        en.agentRun.timeline.usage
          .replace("{steps}", "12")
          .replace("{tokens}", new Intl.NumberFormat("en").format(48210)),
      ),
    ).toBeInTheDocument();
  });

  it("writes the tokens the Slovak way in Slovak", async () => {
    await i18n.changeLanguage("sk");
    renderPart(<RunTimeline status="building" steps={12} tokensUsed={48210} />);
    const sk = new Intl.NumberFormat("sk").format(48210);
    expect(sk).not.toBe("48,210");
    expect(screen.getByText(new RegExp(sk.replace(/\s/g, "\\s")))).toBeInTheDocument();
    await i18n.changeLanguage("en");
  });

  it("names every state in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderPart(<RunTimeline status="building" steps={1} tokensUsed={1} />);
      for (const state of RUN_STATES) {
        expect(
          screen.getByText(i18n.t(`agentRun.states.${state}`)),
          `${state} is missing in ${locale}`,
        ).toBeInTheDocument();
      }
    });
  });

  // PF-50: the stream writes the status, so a value the Portal did not write reaches here. It
  // names no step and says nothing stopped, and none of it becomes markup.
  it("draws nothing at all from a status the Portal did not write", () => {
    const { container } = renderPart(
      <RunTimeline status="<img src=x onerror=alert(1)>" steps={0} tokensUsed={0} />,
    );
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(
      screen.getAllByRole("listitem").filter((item) => item.hasAttribute("aria-current")),
    ).toHaveLength(0);
    expect(container.textContent).not.toContain("onerror");
    expect(document.querySelector("img")).toBeNull();
  });

  it.each(["building", "failed", "published"] as const)(
    "has no axe violations for a %s run",
    async (status) => {
      const { container } = renderPart(
        <RunTimeline status={status} steps={4} tokensUsed={2048} />,
      );
      await expectNoAxeViolations(container);
    },
  );
});
