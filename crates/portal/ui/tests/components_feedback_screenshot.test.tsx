// The feedback dialog's screenshot when the browser shares the tab: the frame drawn, every field
// painted over, the stream stopped; a refusal named; and the dialog closed by Cancel and Escape.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { FeedbackButton } from "../src/components/FeedbackButton";

function renderButton() {
  const sent: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      sent.push(request);
      return new Response(JSON.stringify({ id: 7 }), { status: 202, headers: { "Content-Type": "application/json" } });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <input aria-label="Private" defaultValue="secret-free text" />
        <FeedbackButton />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return sent;
}

/** A tab the browser shares: a 1600 px frame and a canvas that records what was painted. */
function shareTab(context: unknown) {
  const stopped: string[] = [];
  const getDisplayMedia = vi.fn(async () => ({ getTracks: () => [{ stop: () => stopped.push("video") }] }));
  vi.stubGlobal("navigator", { ...navigator, mediaDevices: { getDisplayMedia } });
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
  vi.spyOn(HTMLVideoElement.prototype, "videoWidth", "get").mockReturnValue(1600);
  vi.spyOn(HTMLVideoElement.prototype, "videoHeight", "get").mockReturnValue(900);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue("data:image/png;base64,AAAA");
  vi.spyOn(HTMLInputElement.prototype, "getBoundingClientRect").mockReturnValue({ left: 10, top: 10, width: 100, height: 20, right: 110, bottom: 30, x: 10, y: 10, toJSON: () => ({}) });
  return { stopped, getDisplayMedia };
}

async function openAndTick(text: string) {
  await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
  const dialog = await screen.findByRole("dialog", { name: en.feedback.title });
  await userEvent.type(within(dialog).getByLabelText(en.feedback.text), text);
  await userEvent.click(within(dialog).getByLabelText(en.feedback.screenshot));
  await userEvent.click(within(dialog).getByRole("button", { name: en.feedback.send }));
}

describe("feedback screenshot", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("sends the shared frame with the page's fields painted over and stops the stream", async () => {
    const painted: number[][] = [];
    const context = { drawImage: vi.fn(), fillRect: (...rect: number[]) => painted.push(rect), fillStyle: "" };
    const { stopped, getDisplayMedia } = shareTab(context);
    const sent = renderButton();
    await openAndTick("The map is empty");
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(getDisplayMedia).toHaveBeenCalledOnce();
    expect(((await sent[0].clone().json()) as { screenshot?: string }).screenshot).toBe("data:image/png;base64,AAAA");
    expect(context.drawImage).toHaveBeenCalled();
    expect(painted.length).toBeGreaterThan(0);
    expect(stopped).toEqual(["video"]);
  });

  it("calls a canvas without a 2D context unsupported, stops the stream and sends nothing", async () => {
    const { stopped } = shareTab(null);
    const sent = renderButton();
    await openAndTick("Broken");
    expect(await screen.findByRole("alert")).toHaveTextContent(en.feedback.screenshotUnsupported);
    expect(stopped).toEqual(["video"]);
    expect(sent).toHaveLength(0);
  });

  it("names a refused share and sends nothing", async () => {
    vi.stubGlobal("navigator", { ...navigator, mediaDevices: { getDisplayMedia: () => Promise.reject(new DOMException("denied", "NotAllowedError")) } });
    const sent = renderButton();
    await openAndTick("Refused");
    expect(await screen.findByRole("alert")).toHaveTextContent(en.feedback.screenshotRefused);
    expect(sent).toHaveLength(0);
  });

  it("closes on Cancel and on Escape", async () => {
    renderButton();
    await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
    const dialog = await screen.findByRole("dialog", { name: en.feedback.title });
    await userEvent.click(within(dialog).getByRole("button", { name: en.form.cancel }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
    await screen.findByRole("dialog", { name: en.feedback.title });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
