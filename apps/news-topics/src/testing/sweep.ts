import { act, fireEvent, waitFor } from "@testing-library/react";

const nameOf = (element: HTMLElement) => element.getAttribute("aria-label") ?? element.textContent ?? "";

/**
 * Every control under `root` used once, as a reader trying each would: each choice of each list
 * chosen and the list put back, each text field typed into and put back, each box ticked (a filter
 * it sets shows the button that clears it), then each enabled button
 * and link pressed once by its name (a press may change what is shown, so the next is found anew),
 * and the lists and fields a press showed used last. The shell's header is left alone: its language
 * list reloads the page.
 */
export async function sweep(root: HTMLElement): Promise<void> {
  await fields(root);
  const pressed = new Set<string>();
  const unpressed = () =>
    [...root.querySelectorAll<HTMLElement>("button, a[href]")].find(
      (control) => outside(control) && !(control as HTMLButtonElement).disabled && !pressed.has(nameOf(control)),
    );
  for (;;) {
    // A press answered by the server opens its controls again a moment later.
    const next =
      unpressed() ??
      (await waitFor(
        () => {
          const found = unpressed();
          if (!found) throw new Error("no control left");
          return found;
        },
        { timeout: 200 },
      ).catch(() => undefined));
    if (!next) break;
    pressed.add(nameOf(next));
    await act(async () => {
      fireEvent.click(next);
    });
  }
  await fields(root);
}

const outside = (element: Element) => element.closest("header") === null;

// Each field is found anew: a choice may draw the fields after it again (a station's own form).
async function fields(root: HTMLElement): Promise<void> {
  for (let i = 0; ; i++) {
    const select = [...root.querySelectorAll("select")].filter(outside)[i];
    if (!select) break;
    const before = select.value;
    for (const value of [...[...select.options].map((o) => o.value), before]) {
      await act(async () => {
        fireEvent.change([...root.querySelectorAll("select")].filter(outside)[i] ?? select, { target: { value } });
      });
    }
  }
  for (let i = 0; ; i++) {
    const input = [...root.querySelectorAll("input")].filter(outside)[i];
    if (!input) break;
    if (input.type === "checkbox") {
      await act(async () => {
        fireEvent.click(input);
      });
      continue;
    }
    const before = input.value;
    await act(async () => {
      fireEvent.change(input, { target: { value: `${before}1` } });
      fireEvent.change(input, { target: { value: before } });
    });
  }
}
