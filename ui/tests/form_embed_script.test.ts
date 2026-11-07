/**
 * T-3266, API/01 §33: `/f/embed.js` puts a public form's page in a frame after its own tag and
 * sizes the frame from that page's height message alone.
 */
// covers: public/f/embed.js.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";

const SOURCE = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../public/f/embed.js"), "utf8");

function load(attributes: Record<string, string>): HTMLScriptElement {
  const tag = document.createElement("script");
  tag.src = "https://portal.city.example/f/embed.js";
  for (const [name, value] of Object.entries(attributes)) tag.setAttribute(name, value);
  document.body.append(tag);
  Object.defineProperty(document, "currentScript", { value: tag, configurable: true });
  new Function(SOURCE)();
  return tag;
}

afterEach(() => {
  document.body.replaceChildren();
  Object.defineProperty(document, "currentScript", { value: null, configurable: true });
  vi.restoreAllMocks();
});

describe("the form embed script", () => {
  it("inserts the form's frame after its tag and sizes it from the form's own message", () => {
    const tag = load({ "data-jc-form": "zt4qm7ge2xdv6ksb3ncf5arw2y", "data-jc-title": "Report a pothole" });
    const frame = tag.nextElementSibling as HTMLIFrameElement;
    expect(frame.tagName).toBe("IFRAME");
    expect(frame.src).toBe("https://portal.city.example/f/zt4qm7ge2xdv6ksb3ncf5arw2y");
    expect(frame.title).toBe("Report a pothole");

    const send = (data: unknown, origin: string, source: MessageEventSource | null) =>
      window.dispatchEvent(new MessageEvent("message", { data, origin, source }));
    send({ type: "jc-form-height", height: 812.4 }, "https://portal.city.example", frame.contentWindow);
    expect(frame.style.height).toBe("813px");
    // Another origin, another window, another message or an absurd height changes nothing.
    send({ type: "jc-form-height", height: 10 }, "https://evil.example", frame.contentWindow);
    send({ type: "jc-form-height", height: 10 }, "https://portal.city.example", window);
    send({ type: "other", height: 10 }, "https://portal.city.example", frame.contentWindow);
    send({ type: "jc-form-height", height: "10" }, "https://portal.city.example", frame.contentWindow);
    expect(frame.style.height).toBe("813px");
    send({ type: "jc-form-height", height: 1e9 }, "https://portal.city.example", frame.contentWindow);
    expect(frame.style.height).toBe("20000px");
  });

  it("inserts nothing for a tag that names no form slug", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    for (const slug of ["", "../../evil", "a b", "ABC\"onload=x"]) {
      const tag = load({ "data-jc-form": slug });
      expect(tag.nextElementSibling).toBeNull();
    }
    expect(error).toHaveBeenCalledTimes(4);
  });
});
