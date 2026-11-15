import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccessProvider } from "../access";
import { memory, STEWARD, VIEWER } from "../fixtures/memory";
import { ServerProblem } from "../plans";
import type { Me } from "../plans";
import { LevelsCard, levelsOf } from "./LevelsCard";

function show(me: Me | null, api = memory(me), lang: "en" | "fi" | "sv" = "en") {
  render(
    <AccessProvider api={api}>
      <LevelsCard lang={lang} />
    </AccessProvider>,
  );
  return api;
}

const field = (name: string) => screen.getByRole("spinbutton", { name });

// T-3826: the planner's levels are one set for every plan; a steward changes them, a viewer reads.
describe("LevelsCard", () => {
  it("reads whole percents as shares only in their order", () => {
    expect(levelsOf({ low: "15", target: "50", high: "85" })).toEqual({ low: 0.15, target: 0.5, high: 0.85 });
    expect(levelsOf({ low: "0", target: "1", high: "100" })).toEqual({ low: 0, target: 0.01, high: 1 });
    expect(levelsOf({ low: "50", target: "50", high: "85" })).toBeNull();
    expect(levelsOf({ low: "10", target: "50", high: "101" })).toBeNull();
    expect(levelsOf({ low: "", target: "50", high: "85" })).toBeNull();
    expect(levelsOf({ low: "1.5", target: "50", high: "85" })).toBeNull();
    expect(levelsOf({ low: "-1", target: "50", high: "85" })).toBeNull();
  });

  it("lets a steward save new levels, shown at once and kept by the server", async () => {
    const api = show(STEWARD);
    await waitFor(() => expect(field("About to run empty below (%)")).toBeEnabled());
    fireEvent.change(field("About to run empty below (%)"), { target: { value: "20" } });
    fireEvent.change(field("Bring back to (%)"), { target: { value: "55" } });
    fireEvent.change(field("About to be full above (%)"), { target: { value: "80" } });
    fireEvent.click(screen.getByRole("button", { name: "Save the levels" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The levels are saved. The next plans use them."));
    expect(api.setLevels).toHaveBeenCalledWith({ low: 0.2, target: 0.55, high: 0.8 });
    expect(api.kept).toEqual({ low: 0.2, target: 0.55, high: 0.8 });
    expect(field("About to run empty below (%)")).toHaveValue(20);
  });

  // T-4277: the form opened one commit before the kept levels were copied into it, so the first
  // value a steward typed could be overwritten by them (ci-full 38862786573 saved low 0.15).
  it("opens the form already holding the kept levels, never the defaults first", async () => {
    const api = memory(STEWARD);
    vi.mocked(api.levels).mockResolvedValue({ low: 0.1, target: 0.6, high: 0.9 });
    const seen: string[] = [];
    // Read in the commit that enables the field, before any effect after it runs.
    const watch = new MutationObserver(() => {
      const input = screen.queryByRole("spinbutton", { name: "About to run empty below (%)" }) as HTMLInputElement | null;
      if (input && !input.disabled && seen.length === 0) seen.push(input.value);
    });
    watch.observe(document.body, { subtree: true, attributes: true, childList: true });
    show(STEWARD, api);
    await waitFor(() => expect(seen).toHaveLength(1));
    watch.disconnect();
    expect(seen).toEqual(["10"]);
  });

  it("refuses levels out of order before anything is sent, and says the rule", async () => {
    const api = show(STEWARD);
    await waitFor(() => expect(field("About to run empty below (%)")).toBeEnabled());
    fireEvent.change(field("About to run empty below (%)"), { target: { value: "60" } });
    expect(screen.getByRole("button", { name: "Save the levels" })).toBeDisabled();
    expect(field("About to run empty below (%)")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText(/The levels go 0 ≤ empty below/)).toBeInTheDocument();
    fireEvent.submit(screen.getByRole("form", { name: "The planner's levels for every station" }));
    expect(api.setLevels).not.toHaveBeenCalled();
  });

  it("puts the levels back as they were when the server refuses, and says so", async () => {
    const api = memory(STEWARD);
    api.setLevels = vi.fn(async () => {
      throw new ServerProblem(400, "the levels need 0 ≤ empty below < bring back to < full above ≤ 1");
    });
    show(STEWARD, api);
    await waitFor(() => expect(field("About to run empty below (%)")).toBeEnabled());
    fireEvent.change(field("About to run empty below (%)"), { target: { value: "20" } });
    fireEvent.click(screen.getByRole("button", { name: "Save the levels" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The levels were not saved; the earlier ones stand: the levels need"));
    expect(field("About to run empty below (%)")).toHaveValue(15);
  });

  it("shows a viewer the levels with the form closed, and says who may change them", async () => {
    const api = show(VIEWER);
    await waitFor(() => expect(api.me).toHaveBeenCalled());
    expect(field("About to run empty below (%)")).toBeDisabled();
    expect(field("About to run empty below (%)")).toHaveValue(15);
    const group = screen.getByRole("group");
    expect(group).toHaveAttribute("title", "Only a steward changes the levels.");
    expect(group).toHaveAccessibleDescription("Only a steward changes the levels.");
  });

  it("speaks Swedish", async () => {
    show(null, memory(null), "sv");
    expect(await screen.findByText("Endast en förvaltare ändrar nivåerna.")).toBeInTheDocument();
    expect(field("Snart tom under (%)")).toBeDisabled();
  });
});
