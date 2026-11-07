import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let lastRendered: any = null;
const mockRender = vi.fn((element) => {
  lastRendered = element;
});

vi.mock("react-dom/client", () => ({
  createRoot: vi.fn(() => ({
    render: mockRender,
  })),
}));

import { ConfigError } from "../src/sdk/config";
import { announceReady, startApp } from "../src/sdk/start";

function DummyApp() {
  return <div>App</div>;
}

describe("startApp", () => {
  beforeEach(() => {
    lastRendered = null;
    mockRender.mockClear();
    document.body.innerHTML = `
      <div id="root"></div>
      <script id="jc-config" type="application/json">
        {
          "slug": "demo",
          "orgDomain": "example.org",
          "space": "demo",
          "transport": "bridge"
        }
      </script>
    `;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads #jc-config, applies tokens, renders into #root, and returns client", () => {
    const { client, tokens } = startApp(DummyApp, {
      tokens: { color: { accent: "#ff0000" } },
    });

    expect(client.config.slug).toBe("demo");
    expect(tokens.color.accent).toBe("#ff0000");
    expect(document.documentElement.style.getPropertyValue("--jc-color-accent")).toBe("#ff0000");
    expect(mockRender).toHaveBeenCalledTimes(1);
    expect(lastRendered).not.toBeNull();
  });

  it("throws when #root is missing", () => {
    const rootEl = document.getElementById("root");
    rootEl?.remove();

    expect(() => startApp(DummyApp)).toThrow("No #root element");
  });

  it("renders Problem and throws ConfigError when config is invalid", () => {
    const scriptEl = document.getElementById("jc-config");
    scriptEl!.textContent = "{ invalid json }";

    expect(() => startApp(DummyApp)).toThrow(ConfigError);
    expect(mockRender).toHaveBeenCalledTimes(1);
  });
});

describe("a published App tells the page that frames it that it is up (T-2941)", () => {
  const framed = () => {
    const postMessage = vi.fn();
    return { win: { parent: { postMessage } } as unknown as Window, postMessage };
  };

  it("posts one data-free ready message to the parent when framed", () => {
    const { win, postMessage } = framed();
    announceReady(win);
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage).toHaveBeenCalledWith({ kind: "jc-ready" }, "*");
  });

  it("posts nothing when the App is the top window", () => {
    const postMessage = vi.fn();
    const win = { postMessage } as unknown as Window & { parent: Window };
    win.parent = win;
    announceReady(win);
    expect(postMessage).not.toHaveBeenCalled();
  });

  it("startApp announces a published (origin) App to its frame once", () => {
    const parent = { postMessage: vi.fn() };
    vi.spyOn(window, "parent", "get").mockReturnValue(parent as unknown as Window);
    document.body.innerHTML = `
      <div id="root"></div>
      <script id="jc-config" type="application/json">
        { "slug": "demo", "orgDomain": "example.org", "space": "demo", "transport": "origin", "appName": "demo-app" }
      </script>
    `;
    startApp(DummyApp);
    expect(parent.postMessage).toHaveBeenCalledTimes(1);
    expect(parent.postMessage).toHaveBeenCalledWith({ kind: "jc-ready" }, "*");
  });
});
