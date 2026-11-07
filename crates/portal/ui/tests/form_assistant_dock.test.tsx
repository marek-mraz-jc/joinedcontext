/**
 * T-1749: AssistantDock against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The dock was eleven controls hand-rolled off one class string, a bare composer outside `Field`,
 * and a start failure printed at caption size in a paragraph nothing pointed at: the send failed
 * and the box the person was still sitting in said nothing. The closed bubble advertised
 * `aria-controls="run-chat"` while no such element exists until the panel opens. The cases below
 * are those, then `checkForm` (T-1730) on the composer and the contract's axe, keyboard and
 * locale runs.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { rememberRun } from "../src/assistant/state";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";
import { checkForm } from "./formContract";

const PROJECT = "helsinki";
const START_PATH = `/api/v1/projects/${PROJECT}/assistant/conversations`;
const REFUSAL = "Your role may not start a conversation in this project; ask its owner for the assistant role.";

function json(body: unknown, status = 200, type = "application/json") {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } });
}

/** The caller may propose everything the example prompts would propose. */
const PERMISSIONS = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    { role: "editor", binding: "editors", scope: "project", rule: { kinds: ["Endpoint", "Dashboard"], verbs: ["read", "propose"] } },
  ],
};

/** The reads the dock makes before anything is asked. */
function reads(url: URL): Response | undefined {
  if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
  if (url.pathname.endsWith("/permissions/me")) return json(PERMISSIONS);
  if (url.pathname.endsWith("/endpoints")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  return undefined;
}

/** Mounts the dock with `start` answering the conversation it starts. */
function renderDock(
  start: () => Response = () => json({ id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V", status: "queued" }, 202),
  launcherSlot: HTMLElement | null = null,
) {
  const started: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      if (request.method === "POST" && url.pathname === START_PATH) {
        started.push(JSON.parse(await request.text()));
        return start();
      }
      return reads(url) ?? json({});
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const rootRoute = createRootRoute({ component: () => <AssistantDock project={PROJECT} launcherSlot={launcherSlot} /> });
  const router = createRouter({ routeTree: rootRoute });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { started };
}

const dock = () => screen.getByRole("complementary", { name: i18n.t("agentRun.conversation.title") });

async function openDock() {
  const person = userEvent.setup();
  await person.click(await screen.findByRole("button", { name: en.assistant.open }));
  await screen.findByLabelText(en.assistant.empty.composer);
  return person;
}

beforeEach(async () => {
  rememberRun(null);
  window.sessionStorage.clear();
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
  rememberRun(null);
});

describe("the closed bubble (UI-16)", () => {
  // UI-16: a relationship to an element that is not in the page leads nowhere.
  it("controls_nothing_that_is_not_in_the_page", async () => {
    renderDock();
    const bubble = await screen.findByRole("button", { name: en.assistant.open });
    const controls = bubble.getAttribute("aria-controls");
    if (controls !== null) {
      expect(document.getElementById(controls), `aria-controls="${controls}" names no element`).not.toBeNull();
    }
    expect(bubble).toHaveAttribute("aria-expanded", "false");
    expect(bubble.className).toContain("focus-ring");
  });
});

describe("the closed bubble on a phone (T-4992)", () => {
  function phoneWidth(phone: boolean) {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: phone && query.includes("max-width"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }));
  }

  it("sits_in_the_header_slot_and_floats_over_no_row", async () => {
    phoneWidth(true);
    const header = document.createElement("div");
    document.body.append(header);
    try {
      renderDock(undefined, header);
      const bubble = await within(header).findByRole("button", { name: en.assistant.open });
      expect(bubble.className).not.toContain("fixed");
      expect(screen.getAllByRole("button", { name: en.assistant.open })).toHaveLength(1);
      await userEvent.click(bubble);
      expect(await screen.findByLabelText(en.assistant.empty.composer)).toBeInTheDocument();
      expect(within(header).queryByRole("button", { name: en.assistant.open })).toBeNull();
    } finally {
      header.remove();
    }
  });

  it("floats_at_the_corner_from_sm_up_even_with_a_slot", async () => {
    phoneWidth(false);
    const header = document.createElement("div");
    document.body.append(header);
    try {
      renderDock(undefined, header);
      const bubble = await screen.findByRole("button", { name: en.assistant.open });
      expect(header).not.toContainElement(bubble);
      expect(bubble.className).toContain("fixed");
    } finally {
      header.remove();
    }
  });
});

describe("the header's buttons (UI-16, UI-44)", () => {
  // UI-16: the shared Button, so the focus ring and the disabled state come from one place.
  it("are_the_shared_button_with_the_portal_focus_ring", async () => {
    renderDock();
    await openDock();
    for (const name of [en.assistant.sideView, en.assistant.floatView, en.assistant.fullScreen, en.assistant.hide, en.assistant.close]) {
      expect(within(dock()).getByRole("button", { name }).className, name).toContain("focus-ring");
    }
  });

  // UI-16: the layout buttons say which one is pressed, and pressing another moves it.
  it("the_layout_buttons_say_which_is_pressed", async () => {
    renderDock();
    const person = await openDock();
    expect(within(dock()).getByRole("button", { name: en.assistant.sideView })).toHaveAttribute("aria-pressed", "true");
    await person.click(within(dock()).getByRole("button", { name: en.assistant.floatView }));
    expect(within(dock()).getByRole("button", { name: en.assistant.floatView })).toHaveAttribute("aria-pressed", "true");
    expect(within(dock()).getByRole("button", { name: en.assistant.sideView })).toHaveAttribute("aria-pressed", "false");
  });

  // UI-15: the translated title may wrap; it is not `shrink-0` beside a name that truncates.
  it("the_title_gives_way_to_the_buttons_in_a_long_language", async () => {
    await i18n.changeLanguage("de");
    renderDock();
    await userEvent.click(await screen.findByRole("button", { name: i18n.t("assistant.open") }));
    const title = await screen.findByRole("heading", { name: i18n.t("assistant.title") });
    expect(title.className).not.toContain("shrink-0");
  });
});

describe("the composer (UI-04, UI-44, UI-48)", () => {
  // UI-44: the refusal is the composer's own error — invalid, described, announced — and the text stays.
  it("a_refused_start_marks_the_box_and_keeps_every_word", async () => {
    renderDock(() => json({ title: "Forbidden", status: 403, detail: REFUSAL }, 403, "application/problem+json"));
    const person = await openDock();
    const box = screen.getByLabelText(en.assistant.empty.composer);
    await person.type(box, "Which stations were empty yesterday?");
    await person.click(within(dock()).getByRole("button", { name: en.assistant.empty.send }));
    await waitFor(() => expect(box).toHaveAttribute("aria-invalid", "true"));
    expect(box).toHaveAccessibleDescription(new RegExp(REFUSAL.slice(0, 40)));
    expect(within(dock()).getByRole("alert")).toHaveTextContent(REFUSAL);
    expect(box).toHaveValue("Which stations were empty yesterday?");
  });

  // On dev the project's daily run quota refused the start and the box said "Check the message
  // and try again": no message would have started it. The reason is said, and only the reason.
  it("a_start_refused_by_the_quota_says_why_and_blames_no_message", async () => {
    const quota = "quota: agentRunsPerDay 51 of 50 in project helsinki; raise the quota on the project or the organization first";
    renderDock(() => json({ title: "Forbidden", status: 403, detail: quota }, 403, "application/problem+json"));
    const person = await openDock();
    await person.type(screen.getByLabelText(en.assistant.empty.composer), "Which stations are empty?");
    await person.click(within(dock()).getByRole("button", { name: en.assistant.empty.send }));
    const alert = await within(dock()).findByRole("alert");
    expect(alert).toHaveTextContent(`${en.assistant.empty.failed} ${quota}`);
    expect(alert.textContent).not.toMatch(/check the message/i);
  });

  // UI-48: a start in flight shows it, and a second Enter sends nothing more.
  it("a_start_in_flight_is_busy_and_sends_once", async () => {
    let answer: (response: Response) => void = () => {};
    const { started } = renderDock();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        const url = new URL(request.url, "http://localhost");
        if (request.method === "POST" && url.pathname === START_PATH) {
          started.push(JSON.parse(await request.text()));
          return new Promise<Response>((resolve) => {
            answer = resolve;
          });
        }
        return reads(url) ?? json({});
      }),
    );
    const person = await openDock();
    const box = screen.getByLabelText(en.assistant.empty.composer);
    await person.type(box, "air quality in Kallio{Enter}");
    await person.type(box, "{Enter}");
    const send = within(dock()).getByRole("button", { name: en.assistant.empty.send });
    expect(send).toHaveAttribute("aria-busy", "true");
    expect(started).toHaveLength(1);
    expect(started[0]).toMatchObject({ message: "air quality in Kallio", endpointNames: [] });
    await act(async () => {
      answer(json({ title: "Unavailable", status: 503, detail: "The model is away." }, 503, "application/problem+json"));
    });
  });
});

describe("the dock meets the UI contract", () => {
  // UI-04, UI-15, UI-16, UI-44, UI-48: the form contract on the composer, rule by rule (T-1730).
  it("meets_the_form_contract", async () => {
    await checkForm(
      () => {
        rememberRun(null);
        // The dock is always beside a page; the closed bubble alone has no text, and the harness
        // waits for the page to paint some.
        return (
          <main>
            <h1>Air quality</h1>
            <AssistantDock project={PROJECT} />
          </main>
        );
      },
      {
        fields: [{ label: en.assistant.empty.composer, value: "Which stations were empty yesterday?" }],
        submit: en.assistant.empty.send,
        submitPath: START_PATH,
        refusal: REFUSAL,
        answer: (url) => reads(url),
        open: async (user) => {
          await user.click(await screen.findByRole("button", { name: en.assistant.open }));
          await screen.findByLabelText(en.assistant.empty.composer);
        },
      },
    );
  }, 60_000);

  // UI-16: axe finds nothing in the open dock, with a refusal on screen, in both themes.
  it.each(["light", "dark"])("has_no_axe_violation_in_the_%s_theme", async (theme) => {
    document.documentElement.dataset.theme = theme;
    renderDock(() => json({ title: "Forbidden", status: 403, detail: REFUSAL }, 403, "application/problem+json"));
    const person = await openDock();
    await expectNoViolations(dock());
    await person.type(screen.getByLabelText(en.assistant.empty.composer), "air{Enter}");
    await within(dock()).findByRole("alert");
    await expectNoViolations(dock());
    delete document.documentElement.dataset.theme;
  });

  // UI-16: every control is reached by Tab in the order it is drawn.
  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    renderDock();
    const person = await openDock();
    await expectTabOrder(person, dock());
  });

  // UI-15: every string in the four locales, and no key shown in place of a sentence.
  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    renderDock();
    await userEvent.click(await screen.findByRole("button", { name: i18n.t("assistant.open") }));
    await screen.findByLabelText(i18n.t("assistant.empty.composer"));
    expectNoRawKeys(dock());
  });
});
