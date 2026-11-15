import { act, fireEvent } from "@testing-library/react";

const nameOf = (element: HTMLElement) => element.getAttribute("aria-label") ?? element.textContent ?? "";

/**
 * Every control under `root` used once, as a reader trying each would: each choice of each list
 * chosen and the list put back, each text field typed into and put back, then each enabled button
 * and link pressed once by its name (a press may change what is shown, so the next is found anew),
 * and the lists and fields a press showed used last. The shell's header is left alone: its language
 * list reloads the page.
 */
export async function sweep(root: HTMLElement): Promise<void> {
  await fields(root);
  const pressed = new Set<string>();
  for (;;) {
    const next = [...root.querySelectorAll<HTMLElement>("button, a[href]")].find(
      (control) => outside(control) && !(control as HTMLButtonElement).disabled && !pressed.has(nameOf(control)),
    );
    if (!next) break;
    pressed.add(nameOf(next));
    await act(async () => {
      fireEvent.click(next);
    });
  }
  await fields(root);
}

const outside = (element: Element) => element.closest("header") === null;

async function fields(root: HTMLElement): Promise<void> {
  for (const select of [...root.querySelectorAll("select")].filter(outside)) {
    const before = select.value;
    for (const value of [...[...select.options].map((o) => o.value), before]) {
      await act(async () => {
        fireEvent.change(select, { target: { value } });
      });
    }
  }
  for (const input of [...root.querySelectorAll("input")].filter(outside)) {
    const before = input.value;
    await act(async () => {
      fireEvent.change(input, { target: { value: `${before}1` } });
      fireEvent.change(input, { target: { value: before } });
    });
  }
}
