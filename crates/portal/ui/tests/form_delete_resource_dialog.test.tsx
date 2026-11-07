/**
 * T-1751: the DeleteResourceDialog against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The destructive confirm used to be hard-`disabled` with no reason and the Field above it
 * carried neither help nor errors: somebody working by keyboard tabbed from the box straight to
 * Cancel, was never told the Propose button existed, and got no word about what they had typed.
 * The first three cases here are that. `checkForm` is T-1730's and joins this file with it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
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

// The proposal the dialog shows on success renders `ChangeNotice`, which links to the approval
// through the router. The dialog is what is under test, so the link is a plain `<a>` here.
vi.mock("@tanstack/react-router", () => ({
  Link: ({
    children,
    to,
    params,
    className,
  }: {
    children: React.ReactNode;
    to: string;
    params: Record<string, string>;
    className?: string;
  }) => (
    <a
      className={className}
      href={Object.entries(params ?? {}).reduce(
        (path, [key, value]) => path.replace(`$${key}`, value),
        to,
      )}
    >
      {children}
    </a>
  ),
}));

const { DeleteResourceDialog } = await import("../src/components/DeleteResourceDialog");
type ResourceTarget = Parameters<typeof DeleteResourceDialog>[0]["target"];

const NAME = "zvolen-ovzdusie";

const TARGET: ResourceTarget = {
  project: "banskabystrica",
  kind: "Endpoint",
  plural: "endpoints",
  name: NAME,
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-9f8e7d6c", namespace: "banskabystrica" },
  status: { lane: "red", phase: "PendingApproval" },
};

function answer(status: number, body: unknown, problem = false) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": problem ? "application/problem+json" : "application/json" },
  });
}

function renderDialog(respond: (request: Request) => Response = () => answer(200, CHANGE)) {
  const sent: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request =
        input instanceof Request ? input : new Request(String(input), init);
      sent.push(request);
      return respond(request);
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const rendered = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <DeleteResourceDialog target={TARGET} open onOpenChange={() => {}} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...rendered, sent };
}

const dialog = () => screen.findByRole("dialog");
/** The removal itself: the dialog's dry run asking what goes with it is a `DELETE` too (T-3247). */
const isRemoval = (request: Request) => request.method === "DELETE" && !request.url.includes("dryRun");
const propose = async () =>
  within(await dialog()).getByRole("button", { name: en.resourceDelete.propose });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the refusal of a destructive confirm (UI-44)", () => {
  it("the_confirm_is_reachable_while_it_is_refused_and_says_what_is_missing", async () => {
    renderDialog();
    expectDenied(await propose(), `Type ${NAME} to propose its removal.`);
    expect(focusables(await dialog())).toContain(await propose());
  });

  it("a_refused_confirm_sends_nothing_when_it_is_clicked", async () => {
    const { sent } = renderDialog();
    await userEvent.click(await propose());
    expect(sent.filter(isRemoval)).toEqual([]);
  });

  it("the_name_typed_wrong_is_said_beside_the_field_and_the_field_is_marked_invalid", async () => {
    renderDialog();
    const field = within(await dialog()).getByLabelText(`Type ${NAME} to confirm`);
    await userEvent.type(field, "zvolen");
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(field).toHaveAccessibleDescription(new RegExp(`This is not ${NAME}\\.`));
    expect(within(await dialog()).getByRole("alert")).toHaveTextContent(`This is not ${NAME}.`);
  });

  it("an_untouched_field_is_not_an_error_yet", async () => {
    renderDialog();
    const field = within(await dialog()).getByLabelText(`Type ${NAME} to confirm`);
    expect(field).not.toHaveAttribute("aria-invalid", "true");
    expect(field).toHaveAccessibleDescription(en.resourceDelete.exact);
  });

  it("the_matching_name_opens_the_confirm_and_one_delete_is_sent", async () => {
    const { sent } = renderDialog();
    await userEvent.type(within(await dialog()).getByLabelText(`Type ${NAME} to confirm`), NAME);
    expectOpen(await propose());
    await userEvent.click(await propose());
    await waitFor(() =>
      expect(sent.filter(isRemoval)).toHaveLength(1),
    );
    expect(new URL(sent.filter(isRemoval).at(-1)!.url, window.location.origin).pathname).toBe(
      `/api/v1/projects/banskabystrica/endpoints/${NAME}`,
    );
  });

  // PF-58, CC-39: the typed name goes with the removal, so an administrator's is approved with it.
  it("the_typed_name_is_sent_as_the_confirmation", async () => {
    const { sent } = renderDialog();
    await userEvent.type(within(await dialog()).getByLabelText(`Type ${NAME} to confirm`), NAME);
    await userEvent.click(await propose());
    await waitFor(() => expect(sent.some(isRemoval)).toBe(true));
    const removal = sent.find(isRemoval)!;
    expect(new URL(removal.url, window.location.origin).searchParams.get("confirm")).toBe(NAME);
  });
});

describe("what goes with it (T-3247)", () => {
  const DRY = (goesWith?: unknown) => ({
    valid: true,
    lane: "red",
    plan: { create: 0, update: 0, delete: 1, fields: [] },
    ...(goesWith ? { goesWith } : {}),
  });

  it("names_what_leaves_with_it_with_counts_before_the_name_is_typed", async () => {
    const { sent } = renderDialog((request) =>
      request.url.includes("dryRun")
        ? answer(200, DRY([{ what: "rejectedRecords", count: 3 }, { what: "runs", count: 1 }, { what: "grants", count: 2, names: ["Endpoint/app-x", "Policy/app-x-read"] }]))
        : answer(200, CHANGE),
    );
    const section = await within(await dialog()).findByRole("region", { name: en.resourceDelete.goesWith.title });
    expect(section).toHaveTextContent("3 refused records waiting to be fixed and replayed: they do not come back");
    expect(section).toHaveTextContent("the history of 1 run: it does not come back");
    expect(section).toHaveTextContent("Endpoint/app-x, Policy/app-x-read");
    expect(within(await dialog()).getByText(en.resourceDelete.restorable)).toBeInTheDocument();
    // Asking wrote nothing: the one request is the dry run.
    expect(sent.filter(isRemoval)).toEqual([]);
  });

  it("a_resource_that_takes_nothing_shows_no_list_and_still_says_how_it_comes_back", async () => {
    renderDialog((request) => (request.url.includes("dryRun") ? answer(200, DRY()) : answer(200, CHANGE)));
    expect(await within(await dialog()).findByText(en.resourceDelete.restorable)).toBeInTheDocument();
    expect(within(await dialog()).queryByRole("region", { name: en.resourceDelete.goesWith.title })).toBeNull();
  });

  it("a_removal_the_references_block_is_said_before_anyone_types_the_name", async () => {
    renderDialog((request) =>
      request.url.includes("dryRun")
        ? answer(409, { title: "Still referenced", status: 409, detail: "Pipeline air-in reads it." }, true)
        : answer(200, CHANGE),
    );
    expect(await within(await dialog()).findByText("Pipeline air-in reads it.")).toBeInTheDocument();
    expect(within(await dialog()).getByText(en.resourceDelete.referenced)).toBeInTheDocument();
  });
});

describe("what the server refuses is said in the server's words", () => {
  it("a_409_names_what_still_references_it", async () => {
    renderDialog(() =>
      answer(409, { title: "Still referenced", status: 409, detail: "Pipeline air-in reads it." }, true),
    );
    await userEvent.type(within(await dialog()).getByLabelText(`Type ${NAME} to confirm`), NAME);
    await userEvent.click(await propose());
    const alert = await within(await dialog()).findByText("Pipeline air-in reads it.");
    expect(alert).toBeInTheDocument();
    expect(within(await dialog()).getByText(en.resourceDelete.referenced)).toBeInTheDocument();
  });

  it("a_403_is_shown_as_the_server_wrote_it_and_nothing_is_invented", async () => {
    renderDialog(() =>
      answer(403, { title: "Forbidden", status: 403, detail: "Your role may not delete an Endpoint." }, true),
    );
    await userEvent.type(within(await dialog()).getByLabelText(`Type ${NAME} to confirm`), NAME);
    await userEvent.click(await propose());
    expect(
      await within(await dialog()).findByText("Your role may not delete an Endpoint."),
    ).toBeInTheDocument();
    expect(within(await dialog()).queryByText(en.resourceDelete.referenced)).toBeNull();
  });
});

describe("the dialog meets the UI contract", () => {
  it("focus_lands_on_the_field_the_dialog_is_for_without_autofocus", async () => {
    renderDialog();
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(screen.getByRole("dialog")).getByLabelText(`Type ${NAME} to confirm`),
      ),
    );
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    renderDialog();
    await expectTabOrder(user, await dialog());
  });

  it("has_no_axe_violation_when_it_opens_and_when_it_refuses", async () => {
    renderDialog();
    await expectNoViolations(await dialog());
    await userEvent.type(within(await dialog()).getByLabelText(`Type ${NAME} to confirm`), "no");
    await expectNoViolations(await dialog());
  });

  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    renderDialog();
    expectNoRawKeys(await dialog());
  });
});
