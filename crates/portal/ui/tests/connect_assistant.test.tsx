/**
 * T-4114, EP-113: "Connect an AI assistant", the hub's URL and one ready line per MCP client.
 * Every line names the hub alone, so one installation serves every Endpoint and no slug, token
 * or key is ever in it (EP-110).
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ConnectAssistant, mcpInstall } from "../src/components/endpoints/ConnectAssistant";

const HUB = "https://api.dev.city.example/api/mcp";

describe("mcpInstall", () => {
  it("gives each client the hub under one name", () => {
    const lines = mcpInstall(HUB);
    expect(lines.claude).toBe(`claude mcp add --transport http joinedcontext ${HUB}`);
    expect(JSON.parse(lines.json)).toEqual({ mcpServers: { joinedcontext: { type: "http", url: HUB } } });
    const [scheme, query] = lines.vscode.split("?");
    expect(scheme).toBe("vscode:mcp/install");
    expect(JSON.parse(decodeURIComponent(query))).toEqual({ name: "joinedcontext", type: "http", url: HUB });
    const cursor = new URL(lines.cursor);
    expect(`${cursor.protocol}//${cursor.host}${cursor.pathname}`).toBe("cursor://anysphere.cursor-deeplink/mcp/install");
    expect(cursor.searchParams.get("name")).toBe("joinedcontext");
    expect(JSON.parse(atob(cursor.searchParams.get("config") ?? ""))).toEqual({ url: HUB });
  });

  it("names no endpoint and no credential in any line", () => {
    for (const line of Object.values(mcpInstall(HUB))) {
      const plain = decodeURIComponent(line);
      expect(plain).not.toMatch(/endpoint\/|token|bearer|secret/i);
    }
  });
});

describe("ConnectAssistant", () => {
  const writeText = vi.fn(() => Promise.resolve());

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  });

  afterEach(() => {
    writeText.mockClear();
  });

  function renderPanel(domain: string | undefined) {
    render(
      <I18nextProvider i18n={i18n}>
        <ConnectAssistant domain={domain} />
      </I18nextProvider>,
    );
  }

  it("shows the hub, the install links and copies each client's line", async () => {
    renderPanel("dev.city.example");
    expect(screen.getByRole("heading", { name: en.endpoints.hub.title })).toBeInTheDocument();
    expect(screen.getByText(HUB)).toBeInTheDocument();
    const lines = mcpInstall(HUB);
    expect(screen.getByRole("link", { name: en.endpoints.hub.vscode })).toHaveAttribute("href", lines.vscode);
    expect(screen.getByRole("link", { name: en.endpoints.hub.cursor })).toHaveAttribute("href", lines.cursor);

    await userEvent.click(screen.getByRole("button", { name: en.endpoints.copyUrl }));
    expect(writeText).toHaveBeenLastCalledWith(HUB);
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.hub.claude }));
    expect(writeText).toHaveBeenLastCalledWith(lines.claude);
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.hub.json }));
    expect(writeText).toHaveBeenLastCalledWith(lines.json);
  });

  it("lets the keyboard reach and scroll each code line, each under its own name (T-4973)", async () => {
    renderPanel("dev.city.example");
    const lines = mcpInstall(HUB);
    for (const [name, text] of [
      [en.endpoints.hub.claudeLine, lines.claude],
      [en.endpoints.hub.jsonLine, lines.json],
    ]) {
      const block = screen.getByLabelText(name);
      expect(block.tagName).toBe("PRE");
      expect(block).toHaveTextContent(text.split("\n")[0]);
      document.body.focus();
      for (let step = 0; step < 10 && document.activeElement !== block; step++) await userEvent.tab();
      expect(block).toHaveFocus();
    }
  });

  it("falls back to the page's origin when the installation names no host", () => {
    renderPanel(undefined);
    expect(screen.getByText(`${window.location.origin}/api/mcp`)).toBeInTheDocument();
  });
});
