/**
 * T-1755: the WorkOnCopyDialog against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * Four things this file holds, each of which was wrong on main: the trigger had no permission
 * handling at all, so the refusal arrived after the work as a raw server sentence; the name's
 * refusal was a loose `<p role="alert">` the Input was never tied to; the optional second box
 * borrowed the dialog's own heading for its label; and "Covers" was a `Field` label pointing at
 * an id no control had. `checkForm` is T-1730's and joins this file with it.
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
} from "./checks";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: ReactNode; to: string; params?: Record<string, string> }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)}>{children}</a>
  ),
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search: {} } }),
}));

vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated",
  useAuth: () => ({ identity: { email: "jana@hel.fi", username: "jana" }, status: "authenticated" }),
}));

const { WorkOnCopyAction, WorkOnCopyDialog } = await import("../src/components/WorkOnCopyDialog");

const PROJECT = "helsinki";

/** The grants `permissions/me` answers with; `null` is the document that lists none. */
let grants: { rule: { kinds: string[]; verbs: string[] } }[] = [
  { rule: { kinds: ["ContextSpace"], verbs: ["propose"] } },
];
let posted: { path: string; body: unknown }[];
let answerPost: () => Response;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
  });

beforeEach(async () => {
  await i18n.changeLanguage("en");
  posted = [];
  grants = [{ rule: { kinds: ["ContextSpace"], verbs: ["propose"] } }];
  answerPost = () => json({ name: "copy", project: PROJECT }, 201);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request =
        input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, grants });
      }
      posted.push({
        path: url.pathname,
        body: await request.clone().json().catch(() => undefined),
      });
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

const dialog = () =>
  show(<WorkOnCopyDialog project={PROJECT} scope={{ kind: "project" }} open onOpenChange={() => {}} />);

const start = () => screen.getByRole("button", { name: en.workspaces.open.submit });

describe("the refusal arrives before the work, not after it (UI-44)", () => {
  it("a_role_that_may_propose_nothing_cannot_open_a_copy_and_is_told_why", async () => {
    grants = [];
    show(<WorkOnCopyAction project={PROJECT} scope={{ kind: "project" }} />);
    const trigger = await screen.findByRole("button", { name: en.workspaces.open.action });
    await waitFor(() => expectDenied(trigger, en.workspaces.open.denied));
  });

  it("a_refused_trigger_opens_no_dialog_and_posts_nothing", async () => {
    grants = [];
    show(<WorkOnCopyAction project={PROJECT} scope={{ kind: "project" }} />);
    const trigger = await screen.findByRole("button", { name: en.workspaces.open.action });
    await waitFor(() => expectDenied(trigger));
    await userEvent.click(trigger);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(posted).toEqual([]);
  });

  it("a_role_that_may_propose_something_opens_it", async () => {
    show(<WorkOnCopyAction project={PROJECT} scope={{ kind: "project" }} />);
    const trigger = await screen.findByRole("button", { name: en.workspaces.open.action });
    await waitFor(() => expectOpen(trigger));
    await userEvent.click(trigger);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
});

describe("the name the copy is given", () => {
  it("a_refused_name_marks_the_field_and_is_read_out_with_it", async () => {
    dialog();
    const name = screen.getByLabelText(/^Name/);
    await userEvent.clear(name);
    await userEvent.type(name, "Čistenie");
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(name).toHaveAccessibleDescription(
      new RegExp(en.workspaces.open.nameInvalid.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
    );
  });

  it("the_hint_is_read_before_anything_is_wrong", () => {
    dialog();
    expect(screen.getByLabelText(/^Name/)).toHaveAccessibleDescription(
      en.workspaces.open.nameHint,
    );
  });

  it("the_start_button_is_refused_and_reachable_while_the_name_is_not_a_label", async () => {
    dialog();
    await userEvent.clear(screen.getByLabelText(/^Name/));
    expectDenied(start(), en.workspaces.open.nameInvalid);
    await userEvent.click(start());
    expect(posted).toEqual([]);
  });
});

describe("what each box and each line is called", () => {
  it("the_optional_title_has_a_label_of_its_own_not_the_dialogs_heading", () => {
    dialog();
    const title = screen.getByLabelText(en.workspaces.open.titleField);
    expect(title).toBeInstanceOf(HTMLInputElement);
    // The heading says "Work on a copy"; the box used to say it too.
    expect(screen.queryByLabelText(en.workspaces.open.title, { selector: "input" })).toBeNull();
  });

  it("the_close_button_and_the_cancel_button_do_not_answer_to_the_same_name", async () => {
    dialog();
    const box = await screen.findByRole("dialog");
    const named = within(box)
      .getAllByRole("button")
      .map((button) => (button.textContent || button.getAttribute("aria-label") || "").trim());
    expect(new Set(named).size, `two controls answer to one name: ${named.join(", ")}`).toBe(
      named.length,
    );
  });

  it("covers_is_a_heading_over_text_not_a_label_pointing_at_nothing", () => {
    const { container } = dialog();
    expect(screen.getByText(en.workspaces.open.scope).tagName).toBe("P");
    for (const label of Array.from(container.querySelectorAll("label[for]"))) {
      const target = label.getAttribute("for") ?? "";
      expect(container.querySelector(`#${CSS.escape(target)}`), `<label for="${target}"> points at nothing`).not.toBeNull();
    }
  });
});

describe("closing forgets what was typed, whichever way it is closed", () => {
  it("cancel_resets_the_form_the_same_way_the_escape_key_does", async () => {
    let open = true;
    const Harness = () => (
      <WorkOnCopyDialog
        project={PROJECT}
        scope={{ kind: "project" }}
        open={open}
        onOpenChange={(next) => {
          open = next;
        }}
      />
    );
    const { rerender } = show(<Harness />);
    const name = screen.getByLabelText(/^Name/);
    await userEvent.clear(name);
    await userEvent.type(name, "typed-here");
    await userEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: en.form.cancel }),
    );
    expect(open).toBe(false);
    open = true;
    rerender(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <Harness />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(screen.getByLabelText(/^Name/)).toHaveValue("copy");
  });
});

describe("the dialog meets the UI contract", () => {
  it("posts_what_was_typed_once_and_says_the_servers_words_when_it_is_refused", async () => {
    // A taken name is answered at the name (T-3219); any other refusal in the server's words.
    answerPost = () => json({ title: "Forbidden", status: 403, detail: "the project's quota of copies is spent" }, 403);
    dialog();
    await userEvent.click(start());
    expect(await screen.findByRole("alert")).toHaveTextContent("the project's quota of copies is spent");
    expect(posted).toHaveLength(1);
    expect(posted[0].path).toBe(`/api/v1/projects/${PROJECT}/workspaces`);
  });

  it("has_no_axe_violation_open_and_with_the_name_refused", async () => {
    dialog();
    await expectNoViolations(await screen.findByRole("dialog"));
    await userEvent.clear(screen.getByLabelText(/^Name/));
    await userEvent.type(screen.getByLabelText(/^Name/), "Nope");
    await expectNoViolations(await screen.findByRole("dialog"));
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    dialog();
    await expectTabOrder(user, await screen.findByRole("dialog"));
  });

  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    dialog();
    // The scope of a `resources` copy is the API's own `Kind/name` pairs, which no bundle
    // translates; this copy covers the whole project, so nothing here is data.
    expectNoRawKeys(await screen.findByRole("dialog"));
    expect(within(await screen.findByRole("dialog")).getAllByRole("textbox").length).toBe(2);
  });
});

describe("what a new user is not left to guess (T-3219)", () => {
  it("a_name_another_copy_holds_is_answered_at_the_name_and_the_button_says_why", async () => {
    answerPost = () => json({ title: "Conflict", status: 409, detail: "a workspace of that name exists" }, 409);
    dialog();
    await userEvent.click(start());
    const name = screen.getByLabelText(/^Name/);
    const taken = en.workspaces.open.nameTaken.replace("{name}", "copy");
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    expect(name).toHaveAccessibleDescription(new RegExp(taken.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
    expectDenied(start(), taken);
    // A new name clears it.
    await userEvent.type(name, "-2");
    expect(name).not.toHaveAttribute("aria-invalid", "true");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("the_keep_for_choice_says_what_happens_when_the_time_is_up", () => {
    dialog();
    expect(screen.getByLabelText(en.workspaces.open.ttl)).toHaveAccessibleDescription(en.workspaces.open.ttlHint);
  });
});
