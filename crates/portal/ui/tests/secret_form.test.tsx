/**
 * T-5971, MF-24: the secret form a plan's needsSecret links to. The person types the key here,
 * the page sends it once to the secrets door and clears it; a refusal names the field; a secret
 * that exists asks for every key it keeps.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SecretFormPage } from "../src/pages/secrets/SecretFormPage";
import { expectNoAxeViolations, inEveryLocale, json, renderPage } from "./page_contract";

const TYPED = "abc123-typed-in-the-form";
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "pr-7", project: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

function mount(answer: (url: URL, request: Request) => Response | undefined | Promise<Response | undefined>) {
  return renderPage(<SecretFormPage project="helsinki" name="counters-api" secretKey="apiKey" />, {
    answer,
    path: "/projects/helsinki/secrets/new?name=counters-api&key=apiKey",
  });
}

describe("the secret form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.unstubAllGlobals());

  it("names a key typed twice or left blank at its field and sends nothing (F-R2UI4-017)", async () => {
    const sent: unknown[] = [];
    mount(async (url, request) => {
      if (!url.pathname.endsWith("/projects/helsinki/secrets")) return undefined;
      if (request.method === "POST") sent.push(await request.json());
      return json({ items: [] });
    });
    await userEvent.click(await screen.findByRole("button", { name: en.secrets.addKey }));
    await userEvent.click(screen.getByRole("button", { name: en.secrets.addKey }));
    const keys = screen.getAllByRole("textbox", { name: new RegExp(`^${en.secrets.key}`) });
    await userEvent.type(keys[1], "apiKey");
    await userEvent.click(screen.getByRole("button", { name: en.secrets.submit }));
    expect(await screen.findByText(i18n.t("secrets.keyTwice", { key: "apiKey" }))).toBeInTheDocument();
    expect(screen.getByText(en.secrets.keyMissing)).toBeInTheDocument();
    expect(keys[1]).toHaveAttribute("aria-invalid", "true");
    expect(sent).toEqual([]);
  });

  it("sends the typed key once, clears it and shows the change to approve", async () => {
    const sent: unknown[] = [];
    const { container } = mount(async (url, request) => {
      if (!url.pathname.endsWith("/projects/helsinki/secrets")) return undefined;
      if (request.method === "POST") {
        sent.push(await request.json());
        return json(CHANGE, 202);
      }
      return json({ items: [] });
    });
    expect(await screen.findByText(en.secrets.empty)).toBeInTheDocument();
    expect(screen.getByText(en.secrets.neverInChat)).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: new RegExp(en.secrets.name) })).toHaveValue("counters-api");
    expect(screen.getByRole("textbox", { name: new RegExp(`^${en.secrets.key}`) })).toHaveValue("apiKey");
    for (const hint of [en.secrets.hint.name, en.secrets.hint.key, en.secrets.hint.value]) {
      expect(screen.getByText(hint)).toBeInTheDocument();
    }
    const value = screen.getByLabelText(new RegExp(`^${en.secrets.value}`));
    expect(value).toHaveAttribute("type", "password");
    await userEvent.type(value, TYPED);
    await userEvent.click(screen.getByRole("button", { name: en.secrets.submit }));

    expect(await screen.findByText("pr-7")).toBeInTheDocument();
    expect(sent).toEqual([{ name: "counters-api", values: { apiKey: TYPED } }]);
    expect(value).toHaveValue("");
    expect(container.textContent).not.toContain(TYPED);
    await expectNoAxeViolations(container);
  });

  it("names each refused field and asks for every key a stored secret keeps", async () => {
    mount((url, request) => {
      if (!url.pathname.endsWith("/projects/helsinki/secrets")) return undefined;
      if (request.method === "POST") {
        return json(
          {
            type: "https://joinedcontext.com/errors/unprocessable",
            title: "Unprocessable Content",
            status: 422,
            detail: "the secret cannot be written as sent; nothing was kept",
            errors: ["values.apiKey: 1 to 16384 bytes"],
          },
          422,
        );
      }
      return json({
        items: [{ name: "counters-api", keys: ["apiKey", "username"], path: "projects/helsinki/secrets/counters-api.enc.yaml" }],
      });
    });
    expect(await screen.findByText(/apiKey, username/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.secrets.addKept }));
    expect(screen.getAllByRole("textbox", { name: new RegExp(`^${en.secrets.key}`) }).map((el) => (el as HTMLInputElement).value)).toEqual([
      "apiKey",
      "username",
    ]);
    await userEvent.click(screen.getByRole("button", { name: en.secrets.submit }));
    const alert = await screen.findByRole("alert");
    await waitFor(() => expect(alert).toHaveTextContent("values.apiKey"));
  });

  it("keeps its heading and purpose line when the secrets read is refused (T-6048)", async () => {
    const { container } = mount((url) =>
      url.pathname.endsWith("/projects/helsinki/secrets")
        ? json({ type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: "you may not read the secrets of helsinki" }, 403)
        : undefined,
    );
    expect(await screen.findByText(/you may not read the secrets of helsinki/)).toBeInTheDocument();
    const headings = screen.getAllByRole("heading", { level: 1 });
    expect(headings.map((h) => h.textContent)).toEqual([en.secrets.title]);
    expect(headings[0]?.nextElementSibling?.textContent).toBe(en.secrets.description);
    expect(screen.queryByRole("button", { name: en.secrets.submit })).toBeNull();
    await expectNoAxeViolations(container);
  });

  it("has its words in every language", async () => {
    await inEveryLocale(async () => {
      cleanup();
      mount((url) => (url.pathname.endsWith("/secrets") ? json({ items: [] }) : undefined));
      expect(await screen.findByRole("button", { name: i18n.t("secrets.submit") })).toBeInTheDocument();
    });
  });
});
