import { beforeEach, describe, expect, it, vi } from "vitest";
import { resetSelect, startSelect } from "../src/sdk/select";

/** A frame's window whose messages the test sends, from the host page unless `source` says otherwise. */
function frameWindow() {
  const parent = { postMessage: vi.fn() };
  const listeners: ((event: MessageEvent) => void)[] = [];
  const addEventListener = (type: string, listener: (event: MessageEvent) => void) => {
    if (type === "message") listeners.push(listener);
  };
  const send = (data: unknown, source: unknown = parent) => {
    for (const listener of listeners) listener({ data, source } as MessageEvent);
  };
  return { win: { parent, addEventListener } as unknown as Window, parent, send };
}

function click(element: Element): boolean {
  return element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
}

beforeEach(() => {
  resetSelect();
  document.body.innerHTML =
    '<article data-jc-src="src/pages/Card.tsx:3"><h2 data-jc-src="src/pages/Card.tsx:4"><b>Station 7</b></h2><p>free: 3</p></article>';
  document.documentElement.style.cursor = "";
});

describe("startSelect", () => {
  it("posts the nearest stamped position on a click while the host switched it on, and only the position", () => {
    const { win, parent, send } = frameWindow();
    startSelect({ win, doc: document });
    const title = document.querySelector("b") as Element;

    expect(click(title)).toBe(true);
    expect(parent.postMessage).not.toHaveBeenCalled();

    send({ kind: "jc-select-mode", on: true });
    expect(document.documentElement.style.cursor).toBe("crosshair");
    expect(click(title)).toBe(false);
    expect(parent.postMessage).toHaveBeenCalledWith({ kind: "jc-select", src: "src/pages/Card.tsx:4" }, "*");
    click(document.querySelector("p") as Element);
    expect(parent.postMessage).toHaveBeenLastCalledWith({ kind: "jc-select", src: "src/pages/Card.tsx:3" }, "*");

    send({ kind: "jc-select-mode", on: false });
    expect(document.documentElement.style.cursor).toBe("");
    expect(click(title)).toBe(true);
    expect(parent.postMessage).toHaveBeenCalledTimes(2);
  });

  it("ignores a select mode switched on by any window but the host page", () => {
    const { win, parent, send } = frameWindow();
    startSelect({ win, doc: document });
    send({ kind: "jc-select-mode", on: true }, {});
    expect(click(document.querySelector("b") as Element)).toBe(true);
    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it("posts nothing for a click on an element without a stamp, as in a published App", () => {
    document.body.innerHTML = "<h2><b>Station 7</b></h2>";
    const { win, parent, send } = frameWindow();
    startSelect({ win, doc: document });
    send({ kind: "jc-select-mode", on: true });
    click(document.querySelector("b") as Element);
    expect(parent.postMessage).not.toHaveBeenCalled();
  });
});
