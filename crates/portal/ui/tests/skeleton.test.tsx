/**
 * UI-15, UI-16 (T-2137): the grey bar a page shows in the shape of what is coming.
 *
 * A skeleton is decoration. The wait itself is announced once, in words, by the container that
 * holds it (`PageLoading`), so every bar has to stay out of the accessibility tree — a screen
 * reader that read six of them would say nothing six times.
 */
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Skeleton } from "../src/components/ui/Skeleton";
import { expectNoAxeViolations } from "./page_contract";

describe("the skeleton bar", () => {
  it("is hidden from a screen reader and carries no words", () => {
    const { container } = render(<Skeleton />);
    const bar = container.firstElementChild as HTMLElement;
    expect(bar.getAttribute("aria-hidden")).toBe("true");
    expect(bar.textContent).toBe("");
  });

  it("takes the size the page gives it, and keeps its own shape", () => {
    const { container } = render(<Skeleton className="h-40 w-full" />);
    const bar = container.firstElementChild as HTMLElement;
    expect(bar.className).toContain("h-40");
    expect(bar.className).toContain("w-full");
    // The shape is the component's, not the caller's: a bar is a block with a rounded corner
    // and it pulses only where the person has not asked for less motion.
    expect(bar.className).toContain("block");
    expect(bar.className).toContain("rounded-sm");
    expect(bar.className).toContain("motion-safe:animate-pulse");
  });

  it("has no axe violation, alone or several in a row", async () => {
    const { container } = render(
      <div>
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-4 w-96" />
        <Skeleton className="h-40 w-full" />
      </div>,
    );
    await expectNoAxeViolations(container);
  });
});
