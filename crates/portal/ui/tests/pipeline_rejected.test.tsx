/**
 * T-2708: a pipeline's rejected records on the pipelines page (PL-61, ADR-N-034). The row's menu
 * opens them; each says the rule it broke and where, pages go older and back, a person with
 * propose picks records and retries them after a fix, and the answer names the records that stay
 * because a secret in them was masked. A reader sees the list and is told why they cannot retry.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { PipelineRejectedDialog } from "../src/pages/pipelines/PipelineRejected";
import { expectAxeClean, jsonResponse, list, problem, renderRoute } from "./pageHarness";

const PATH = "/projects/helsinki/pipelines";
const REJECTED = "/api/v1/projects/helsinki/pipelines/stations/rejected";

const pipeline = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "stations", namespace: "helsinki" },
  spec: { class: "resident" },
  status: { phase: "Live" },
};

const record = (id: number, over: Record<string, unknown> = {}) => ({
  id,
  at: "2026-09-25T08:00:00Z",
  record: { id: `urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-${id}`, type: "BikeStation" },
  rule: "sh:datatype",
  path: "capacity",
  message: "capacity is not of the slot's datatype xsd:integer",
  ...over,
});

interface Seen {
  retried: unknown[];
}

function answering(seen: Seen, pages: Record<string, unknown>, retry?: unknown) {
  return (path: string, request: Request): Response | undefined => {
    if (path.endsWith("/pipelines")) {
      return jsonResponse(list([pipeline]));
    }
    if (path === `${REJECTED}/retry` && request.method === "POST") {
      void request
        .clone()
        .json()
        .then((body: unknown) => seen.retried.push(body));
      return jsonResponse(retry, 202);
    }
    if (path === REJECTED) {
      const before = new URL(request.url).searchParams.get("before") ?? "newest";
      return jsonResponse(pages[before]);
    }
    return undefined;
  };
}

async function openRejected(): Promise<HTMLElement> {
  await userEvent.click(
    await screen.findByRole("button", { name: en.rowActions.more.replace("{name}", "stations") }),
  );
  await userEvent.click(await screen.findByRole("menuitem", { name: en.pipelines.rejected.open }));
  return screen.findByRole("dialog");
}

describe("a pipeline's rejected records", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists_each_record_with_its_rule_and_path_and_pages_older_and_back", async () => {
    const seen: Seen = { retried: [] };
    await renderRoute({
      path: PATH,
      answer: answering(seen, {
        newest: { items: [record(9), record(8, { rule: "id", path: "", step: 1 })], total: 3, next: 8 },
        "8": { items: [record(3, { rule: "sh:closed", path: "colour" })], total: 3 },
      }),
    });
    const dialog = await openRejected();
    expect(within(dialog).getByText(en.pipelines.rejected.lead)).toBeInTheDocument();
    expect(await within(dialog).findByText("3 records kept (the newest 1000)")).toBeInTheDocument();
    const table = within(dialog).getByRole("table");
    expect(within(table).getAllByRole("row")).toHaveLength(3);
    expect(within(table).getByText("sh:datatype")).toBeInTheDocument();
    expect(within(table).getByText("In step 2")).toBeInTheDocument();
    // T-5516: the record reads as rows, never as JSON text.
    expect(within(table).getByText("urn:ngsi-ld:BikeStation:hel.fi:helsinki:s-9")).toBeInTheDocument();
    expect(table.textContent).not.toContain("{\n");

    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.rejected.older }));
    expect(await within(dialog).findByText("sh:closed")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: en.pipelines.rejected.older })).toBeNull();
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.rejected.newest }));
    expect(await within(dialog).findByText("sh:datatype")).toBeInTheDocument();
  });

  it("opens_the_run_a_record_was_refused_in_with_its_log", async () => {
    const seen: Seen = { retried: [] };
    const older = "2026-09-25T07:00:00Z";
    const runs = {
      items: [
        { run: "2026-09-25T08:00:00Z", firstAt: "2026-09-25T08:00:00Z", lastAt: "2026-09-25T08:01:00Z", sent: 5, rejected: 0, failed: 0 },
        { run: older, firstAt: older, lastAt: older, sent: 3, rejected: 1, failed: 0 },
      ],
    };
    const logged: string[] = [];
    const base = answering(seen, { newest: { items: [record(9, { run: older }), record(8)], total: 2 } });
    await renderRoute({
      path: PATH,
      answer: (path, request) => {
        if (path.endsWith("/pipelines/stations/runs")) return jsonResponse(runs);
        if (path.includes("/pipelines/stations/runs/") && path.endsWith("/log")) {
          logged.push(decodeURIComponent(path.split("/runs/")[1].replace(/\/log$/, "")));
          return jsonResponse({ items: [], total: 0 });
        }
        return base(path, request);
      },
    });
    const dialog = await openRejected();
    const table = within(dialog).getByRole("table");
    expect(await within(dialog).findByText("2 records kept (the newest 1000)")).toBeInTheDocument();
    // Only the record that names its run offers it.
    expect(await within(table).findAllByRole("button", { name: en.pipelines.rejected.openRun })).toHaveLength(1);
    await userEvent.click(within(table).getByRole("button", { name: en.pipelines.rejected.openRun }));

    const runsDialog = await screen.findByRole("dialog");
    await waitFor(() => expect(logged).toContain(older));
    expect(logged).not.toContain("2026-09-25T08:00:00Z");
    const pressed = within(runsDialog).getAllByRole("button", { pressed: true });
    expect(pressed).toHaveLength(1);
  });

  it("retries_the_picked_records_and_names_the_masked_ones_that_stay", async () => {
    const seen: Seen = { retried: [] };
    const { container } = await renderRoute({
      path: PATH,
      answer: answering(
        seen,
        { newest: { items: [record(9), record(8)], total: 2 } },
        { replayed: 1, masked: [8] },
      ),
    });
    const dialog = await openRejected();
    await within(dialog).findByText("2 records kept (the newest 1000)");
    const retry = within(dialog).getByRole("button", { name: "Retry after fix" });
    expect(retry).toHaveAttribute("aria-disabled", "true");
    expect(retry).toHaveAccessibleDescription(en.pipelines.rejected.pickFirst);

    await userEvent.click(within(dialog).getByRole("checkbox", { name: en.pipelines.rejected.pickPage }));
    const ready = within(dialog).getByRole("button", { name: "Retry 2 records after fix" });
    await userEvent.click(ready);

    const status = await within(dialog).findByRole("status");
    expect(status).toHaveTextContent("1 record was replayed through the current model");
    expect(status).toHaveTextContent("1 record stays on the list");
    await waitFor(() => expect(seen.retried).toEqual([{ ids: [9, 8] }]));
    await expectAxeClean(container);
  });

  it("an_empty_list_says_every_record_passed", async () => {
    await renderRoute({
      path: PATH,
      answer: answering({ retried: [] }, { newest: { items: [], total: 0 } }),
    });
    const dialog = await openRejected();
    expect(await within(dialog).findByText(en.pipelines.rejected.empty)).toBeInTheDocument();
    expect(within(dialog).queryByRole("table")).toBeNull();
    // T-4240 (photo audit T-4159 #11): with nothing refused there is nothing to retry, and a
    // coloured Retry button beside "every record passed" read as an action still to take.
    expect(within(dialog).queryByRole("button", { name: /Retry/ })).toBeNull();
  });

  it("a_reader_sees_the_list_and_is_told_why_they_cannot_retry", async () => {
    const seen: Seen = { retried: [] };
    await renderRoute({
      path: PATH,
      permissions: {
        project: "helsinki",
        grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read"] } }],
      },
      answer: answering(seen, { newest: { items: [record(9)], total: 1 } }),
    });
    const dialog = await openRejected();
    await within(dialog).findByText("sh:datatype");
    await userEvent.click(within(dialog).getAllByRole("checkbox")[1]);
    const retry = within(dialog).getByRole("button", { name: "Retry 1 record after fix" });
    expect(retry).toHaveAttribute("aria-disabled", "true");
    expect(retry).toHaveAccessibleDescription(
      i18n.t("permissions.denied", { verb: "propose", kind: "Pipeline" }),
    );
    await userEvent.click(retry);
    expect(seen.retried).toEqual([]);
  });

  it("a_list_that_could_not_be_read_says_why_and_offers_no_retry_of_nothing", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = new URL(input instanceof Request ? input.url : String(input), window.location.origin)
          .pathname;
        return Promise.resolve(
          path.endsWith("/permissions/me")
            ? jsonResponse({ project: "helsinki", bootstrap: true, grants: [] })
            : problem(503, "The rejected list could not be read; try again."),
        );
      }),
    );
    const onClose = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <PipelineRejectedDialog project="helsinki" name="stations" onClose={onClose} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "The rejected list could not be read; try again.",
    );
    expect(within(dialog).queryByRole("button", { name: /Retry/ })).toBeNull();
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});
