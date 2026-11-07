/**
 * T-6208: the (i) beside a space's size says when the broker measured it, and its Recalculate
 * asks the Portal to measure the space again and shows the new answer, or why it could not.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { SpaceUsageInfo } from "../src/components/SpaceUsageInfo";
import type { SpaceUsage } from "../src/api/spaceUsage";
import { expectNoAxeViolations, json, problem, renderPage } from "./page_contract";

const KEPT: SpaceUsage = {
  entities: 40_000,
  broker: "main",
  observedAt: "2026-10-31T16:00:00Z",
  measuredAt: "2026-10-31T16:00:00Z",
  estimated: false,
};

function show(answer: (request: Request) => Response) {
  const asked: string[] = [];
  const view = renderPage(<SpaceUsageInfo project="helsinki" space="air" usage={KEPT} locale="en" />, {
    answer: (url, request) => {
      if (!url.pathname.endsWith("/usage/recalculate")) return undefined;
      asked.push(`${request.method} ${url.pathname}`);
      return answer(request);
    },
  });
  return { ...view, asked };
}

describe("the space usage (i) (T-6208)", () => {
  it("names when the size was measured and how often", async () => {
    const { container } = show(() => json(KEPT));
    const open = await screen.findByTitle("When the size of air was measured");
    await userEvent.click(open);
    expect(screen.getByTestId("space-measured").textContent).toMatch(/^Measured Oct 31, 2026, .* ago\)$/);
    expect(screen.getByText(/measured once an hour/)).toBeTruthy();
    await expectNoAxeViolations(container);
  });

  it("recalculates with one POST and shows the new time", async () => {
    const { asked } = show(() => json({ ...KEPT, entities: 41_000, measuredAt: "2026-10-31T18:30:00Z" }));
    await userEvent.click(await screen.findByTitle("When the size of air was measured"));
    await userEvent.click(screen.getByRole("button", { name: "Recalculate" }));
    await waitFor(() => expect(asked).toEqual(["POST /api/v1/projects/helsinki/spaces/air/usage/recalculate"]));
    expect(await screen.findByText("Measured again.")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says why when the broker could not measure the space", async () => {
    show(() => problem(503, "the broker answered 503 Service Unavailable for the space's counts"));
    await userEvent.click(await screen.findByTitle("When the size of air was measured"));
    await userEvent.click(screen.getByRole("button", { name: "Recalculate" }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/could not be measured: the broker answered 503/)).toBeTruthy();
  });
});

describe("where the (i)'s panel goes (T-6208)", () => {
  const rect = (left: number, top: number) => ({ left, top, right: left + 20, bottom: top + 20, width: 20, height: 20, x: left, y: top, toJSON: () => ({}) }) as DOMRect;
  it("sits under its button and never leaves the screen", async () => {
    const { panelPlace } = await import("../src/components/SpaceUsageInfo");
    expect(panelPlace(rect(1100, 250), 150, { width: 1440, height: 900 })).toEqual({ top: 274, left: 832, width: 288 });
    // a phone: the panel is narrower than the screen and starts at the gutter
    expect(panelPlace(rect(150, 330), 150, { width: 400, height: 900 })).toEqual({ top: 354, left: 96, width: 288 });
    expect(panelPlace(rect(380, 330), 150, { width: 360, height: 900 }).left).toBe(56);
    // no room below: above the button
    expect(panelPlace(rect(1100, 800), 150, { width: 1440, height: 900 }).top).toBe(646);
  });
});

describe("how old a measurement is (T-6208)", () => {
  it("reads in minutes, hours, then days", async () => {
    const { ageText } = await import("../src/components/SpaceUsageInfo");
    const now = new Date("2026-10-31T18:00:00Z");
    expect(ageText(new Date("2026-10-31T17:55:00Z"), now, "en")).toBe("5 minutes ago");
    expect(ageText(new Date("2026-10-31T16:00:00Z"), now, "en")).toBe("2 hours ago");
    expect(ageText(new Date("2026-10-28T18:00:00Z"), now, "en")).toBe("3 days ago");
    expect(ageText(new Date("2026-10-31T18:00:30Z"), now, "en")).toBe("this minute");
  });
});
