import { act, fireEvent } from "@testing-library/react";

const nameOf = (element: HTMLElement) => element.getAttribute("aria-label") ?? element.textContent ?? "";

/**
 * Every control under `root` used once, as a reader trying each would: each enabled button and
 * link pressed once by its name (a press may change what is shown, so the next is found anew), then
 * each choice of each list chosen.
 */
export async function sweep(root: HTMLElement): Promise<void> {
  const pressed = new Set<string>();
  for (;;) {
    const next = [...root.querySelectorAll<HTMLElement>("button, a[href]")].find(
      (control) => !(control as HTMLButtonElement).disabled && !pressed.has(nameOf(control)),
    );
    if (!next) break;
    pressed.add(nameOf(next));
    await act(async () => {
      fireEvent.click(next);
    });
  }
  for (const select of root.querySelectorAll("select")) {
    for (const option of [...select.options].map((o) => o.value)) {
      await act(async () => {
        fireEvent.change(select, { target: { value: option } });
      });
    }
  }
}
