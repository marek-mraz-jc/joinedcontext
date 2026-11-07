/**
 * T-1758: the New project form against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * No test named this file before. The refusal used the exact wrapper `PermissionGuard` was
 * rewritten to remove — a bare `tabIndex={0}` span with no role and no name, around a
 * hard-`disabled` button whose `disabled:pointer-events-none` swallowed the `title`, with a
 * `role="tooltip"` nothing referenced — so a keyboard user landed on an unnamed stop, a mouse
 * user got no tooltip and nobody was told why. And the Open button had no `loading`, so the POST
 * ran with no spinner while the button left the tab order under the person's hands.
 *
 * `checkForm` is T-1730's and joins this file with it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import {
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectOpen,
  expectTabOrder,
  focusables,
} from "./checks";

const navigate = vi.fn();
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: ReactNode; to: string; params?: Record<string, string> }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)}>{children}</a>
  ),
  useNavigate: () => navigate,
}));

const { NewProjectButton, NewProjectDialog, nameProblem } = await import(
  "../src/components/layout/NewProject"
);

const PROJECT = "banskabystrica";
const REFUSED = "Only an organization administrator opens a project here.";

let creation: { allowed: boolean; reason?: string };
let posted: string[];
let answerPost: () => Response;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
  });

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002a", namespace: "org" },
  status: { lane: "yellow", phase: "PendingApproval" },
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  navigate.mockReset();
  posted = [];
  creation = { allowed: true };
  answerPost = () => json(CHANGE, 201);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request =
        input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, grants: [], projects: { creation } });
      }
      posted.push(await request.clone().text());
      return answerPost();
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function show(ui: ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{ui}</I18nextProvider>
    </QueryClientProvider>,
  );
}

const dialog = () => show(<NewProjectDialog open onOpenChange={() => {}} />);
const openButton = () => screen.getByRole("button", { name: en.projects.open });

describe("the sidebar control when the organization refuses (UI-44)", () => {
  it("is_refused_reachable_and_carries_the_apis_own_reason", async () => {
    creation = { allowed: false, reason: REFUSED };
    const { container } = show(<NewProjectButton project={PROJECT} />);
    const button = await screen.findByRole("button", { name: en.projects.new });
    await waitFor(() => expectDenied(button, REFUSED));
    expect(focusables(container)).toContain(button);
  });

  it("wraps_the_button_in_nothing_a_keyboard_can_land_on_that_has_no_name", async () => {
    creation = { allowed: false, reason: REFUSED };
    const { container } = show(<NewProjectButton project={PROJECT} />);
    await waitFor(() =>
      expectDenied(screen.getByRole("button", { name: en.projects.new })),
    );
    expect(container.querySelector('[role="tooltip"]')).toBeNull();
    expect(container.querySelector('span[tabindex="0"]')).toBeNull();
  });

  it("opens_no_dialog_while_it_is_refused", async () => {
    creation = { allowed: false, reason: REFUSED };
    show(<NewProjectButton project={PROJECT} />);
    const button = await screen.findByRole("button", { name: en.projects.new });
    await waitFor(() => expectDenied(button));
    await userEvent.click(button);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("falls_back_to_the_portals_own_words_when_the_api_gave_no_reason", async () => {
    creation = { allowed: false };
    show(<NewProjectButton project={PROJECT} />);
    await waitFor(() =>
      expectDenied(screen.getByRole("button", { name: en.projects.new }), en.projects.notAllowed),
    );
  });

  it("opens_the_dialog_when_the_organization_allows_it", async () => {
    show(<NewProjectButton project={PROJECT} />);
    const button = await screen.findByRole("button", { name: en.projects.new });
    await waitFor(() => expectOpen(button));
    await userEvent.click(button);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("says_what_each_field_is_for_and_names_it_to_a_screen_reader", async () => {
    show(<NewProjectButton project={PROJECT} />);
    const button = await screen.findByRole("button", { name: en.projects.new });
    await waitFor(() => expectOpen(button));
    await userEvent.click(button);
    const open = await screen.findByRole("dialog");
    expect(within(open).getByLabelText(en.projects.displayNameLabel)).toHaveAccessibleDescription(en.projects.displayNameHint);
    expect(within(open).getByLabelText(en.projects.descriptionLabel)).toHaveAccessibleDescription(en.projects.descriptionHint);
  });
});

describe("the name that becomes every path of the project (PF-67)", () => {
  it.each([
    ["", "empty"],
    ["   ", "empty"],
    ["org", "reserved"],
    ["Doprava Mesta", "label"],
    ["-leading", "label"],
    ["trailing-", "label"],
    ["UPPER", "label"],
    ["a".repeat(64), "label"],
    ["doprava", null],
    ["a", null],
    ["a".repeat(63), null],
  ])("nameProblem(%j) is %s", (name, problem) => {
    expect(nameProblem(name)).toBe(problem);
  });

  it("the_refusal_is_beside_the_field_and_the_field_is_marked_invalid", async () => {
    dialog();
    const name = screen.getByLabelText(/^Name/);
    await userEvent.type(name, "Doprava Mesta");
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(name).toHaveAccessibleDescription(new RegExp("starting and ending with"));
  });

  it("the_reserved_namespace_is_refused_in_its_own_words", async () => {
    dialog();
    await userEvent.type(screen.getByLabelText(/^Name/), "org");
    expect(screen.getByRole("alert")).toHaveTextContent(en.projects.nameReserved);
  });

  it("the_open_button_is_refused_reachable_and_sends_nothing", async () => {
    dialog();
    expectDenied(openButton(), en.projects.nameNeeded);
    await userEvent.click(openButton());
    expect(posted).toEqual([]);
  });

  it("a_usable_name_opens_the_button_and_one_project_is_posted", async () => {
    dialog();
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    expectOpen(openButton());
    await userEvent.click(openButton());
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(JSON.parse(posted[0])).toMatchObject({ name: "doprava" });
  });

  it("an_empty_optional_field_is_left_out_rather_than_sent_as_an_empty_string", async () => {
    dialog();
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    await userEvent.type(screen.getByLabelText(en.projects.displayNameLabel), "   ");
    await userEvent.click(openButton());
    await waitFor(() => expect(posted).toHaveLength(1));
    const body = JSON.parse(posted[0]) as Record<string, unknown>;
    expect(body.displayName).toBeUndefined();
    expect(body.description).toBeUndefined();
  });
});

describe("while the project is being opened (UI-15)", () => {
  it("the_button_says_it_is_busy_and_a_second_click_sends_nothing", async () => {
    answerPost = () => json(CHANGE, 201);
    let release: (() => void) | undefined;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request =
          input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        if (new URL(request.url).pathname.endsWith("/permissions/me")) {
          return json({ project: PROJECT, grants: [], projects: { creation } });
        }
        posted.push(await request.clone().text());
        await new Promise<void>((resolve) => {
          release = resolve;
        });
        return json(CHANGE, 201);
      }),
    );
    dialog();
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    await userEvent.click(openButton());
    await waitFor(() => expect(openButton()).toHaveAttribute("aria-busy", "true"));
    // The spinner is drawn, not only announced: the label stays so the width does not jump.
    expect(within(openButton()).getByText(en.projects.open)).toBeTruthy();
    expect(openButton().querySelector("svg")).not.toBeNull();
    await userEvent.click(openButton());
    expect(posted).toHaveLength(1);
    release?.();
  });

  it("a_refusal_from_the_api_is_shown_in_the_apis_own_sentence", async () => {
    answerPost = () => json({ title: "Conflict", status: 409, detail: "doprava is already open." }, 409);
    dialog();
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    await userEvent.click(openButton());
    expect(await screen.findByText("doprava is already open.")).toBeInTheDocument();
  });
});

describe("the form meets the UI contract", () => {
  it("has_no_axe_violation_open_and_with_the_name_refused", async () => {
    dialog();
    await expectNoViolations(await screen.findByRole("dialog"));
    await userEvent.type(screen.getByLabelText(/^Name/), "Nope Nope");
    await expectNoViolations(await screen.findByRole("dialog"));
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    dialog();
    await expectTabOrder(user, await screen.findByRole("dialog"));
  });

  it("focus_lands_on_the_first_field_of_the_form", async () => {
    dialog();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText(/^Name/)));
  });

  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    dialog();
    expectNoRawKeys(await screen.findByRole("dialog"));
    expect(within(await screen.findByRole("dialog")).getAllByRole("textbox")).toHaveLength(3);
  });
});
