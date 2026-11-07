/**
 * T-3272: feedback from any page. The person's words and the page they are on reach the API and
 * nothing that names them; a screenshot only when ticked, and never one the browser was not
 * allowed to take; the fields of the page painted over in the frame.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { FeedbackButton, maskRects } from "../src/components/FeedbackButton";
import { expectNoViolations } from "./checks";

function renderButton(answer: () => Response = () => new Response(JSON.stringify({ id: 7 }), { status: 202, headers: { "Content-Type": "application/json" } })) {
  const sent: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      sent.push(request);
      return answer();
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <FeedbackButton />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return sent;
}

describe("feedback from any page (T-3272)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/approvals?edit=jana#top");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sends the words and the page, and nothing that names the person", async () => {
    const sent = renderButton();
    await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
    const dialog = await screen.findByRole("dialog", { name: en.feedback.title });
    const send = within(dialog).getByRole("button", { name: en.feedback.send });
    expect(send).toHaveAttribute("aria-disabled", "true");
    await expectNoViolations(dialog);

    await userEvent.type(within(dialog).getByLabelText(en.feedback.text), "The Approve button stays grey");
    await userEvent.click(send);
    await waitFor(() => expect(sent).toHaveLength(1));
    const body = (await sent[0].clone().json()) as Record<string, unknown>;
    expect(new URL(sent[0].url).pathname).toBe("/api/v1/feedback");
    expect(body).toEqual({ text: "The Approve button stays grey", page: "/projects/helsinki/approvals" });
    expect(await screen.findByRole("status")).toHaveTextContent(en.feedback.sent);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says the API's refusal in its words and keeps what was typed", async () => {
    renderButton(
      () =>
        new Response(JSON.stringify({ type: "about:blank", title: "Too Many Requests", status: 429, detail: "10 feedbacks in an hour already; send this one in 12 minutes" }), {
          status: 429,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(en.feedback.text), "once more");
    await userEvent.click(within(dialog).getByRole("button", { name: en.feedback.send }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("send this one in 12 minutes");
    expect(within(dialog).getByLabelText(en.feedback.text)).toHaveValue("once more");
  });

  it("takes no screenshot the browser cannot share, and sends nothing then", async () => {
    vi.stubGlobal("navigator", { ...navigator, mediaDevices: {} });
    const sent = renderButton();
    await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(en.feedback.text), "with a picture");
    await userEvent.click(within(dialog).getByLabelText(en.feedback.screenshot));
    await userEvent.click(within(dialog).getByRole("button", { name: en.feedback.send }));
    expect(await screen.findByRole("alert")).toHaveTextContent(en.feedback.screenshotUnsupported);
    expect(sent).toHaveLength(0);
  });

  it("paints over every field in the frame's pixels, and none off screen", () => {
    const rects = maskRects(
      [
        { x: 10, y: 20, width: 100, height: 30 },
        { x: 5000, y: 20, width: 100, height: 30 },
        { x: 10, y: 20, width: 0, height: 30 },
      ],
      2,
      { width: 1600, height: 900 },
    );
    expect(rects).toEqual([{ x: 20, y: 40, width: 200, height: 60 }]);
  });

  it("refuses more than the API keeps before anything is sent", async () => {
    const sent = renderButton();
    await userEvent.click(screen.getByRole("button", { name: en.feedback.button }));
    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByLabelText(en.feedback.text);
    await userEvent.click(field);
    await userEvent.paste("x".repeat(2001));
    expect(field).toHaveAttribute("aria-invalid", "true");
    await userEvent.click(within(dialog).getByRole("button", { name: en.feedback.send }));
    expect(sent).toHaveLength(0);
  });
});
