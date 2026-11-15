/**
 * T-7041: Administration → Open data pages. An administrator reads every page per language
 * (built-in, own, waiting), edits one with the toolbar beside the catalogue renderer's preview,
 * publishes it as a Change or goes back to the built-in text; a 422 marks the field, a 403 and a
 * 409 say what to do, and the history lists only the Changes that wrote page texts. The preview
 * answer is rebuilt as elements: nothing in it runs.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import type { Effective } from "../src/api/permissions";
import { OpendataPages } from "../src/pages/organization/OpendataPages";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { applyFormat, previewElements } from "../src/pages/organization/pageMarkdown";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: { kinds: ["RoleBinding", "Organization"], verbs: ["read", "propose", "approve", "delete"] },
    },
  ],
} as unknown as Effective;

const BUILT_IN = "# Konektor\n\nVstavaný text.";
const text = (page: string, lang: string, markdown?: string) => ({
  page,
  lang,
  title: markdown ? "Náš konektor" : page === "connector" ? "Konektor" : `${page} ${lang}`,
  builtIn: BUILT_IN,
  ...(markdown ? { markdown } : {}),
});

const change = (
  name: string,
  phase: string,
  params: Record<string, unknown> = { kind: "Organization", name: "bbsk" },
) => ({
  apiVersion: "joinedcontext.com/v1",
  kind: "ChangeProposal",
  metadata: { name, namespace: "org" },
  author: { name: "Jana Admin", email: "jana@bbsk.sk" },
  createdAt: "2026-11-15T10:00:00Z",
  decision: phase === "Merged" ? { at: "2026-11-15T11:00:00Z", by: "eva" } : null,
  status: { phase, lane: "yellow", plan: {} },
  summary: { key: "update", params },
});

interface Setup {
  own?: string;
  open?: boolean;
  list?: Response;
  write?: Response;
}

function renderPages(setup: Setup = {}, alone = false) {
  const requests: { method: string; path: string; body?: unknown }[] = [];
  const page = alone ? <OpendataPages /> : <OrganizationPage tab="opendata-pages" anchor="doprava" />;
  const result = renderPage(page, {
    path: "/organization/opendata-pages",
    answer: async (url, request) => {
      const body =
        request.method === "GET"
          ? undefined
          : await request
              .clone()
              .text()
              .then((raw) => (raw ? JSON.parse(raw) : undefined));
      requests.push({ method: request.method, path: url.pathname, body });
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/opendata/pages") {
        return (
          setup.list ??
          json({
            organization: "bbsk",
            languages: ["sk", "en"],
            maxBytes: 16384,
            pages: [
              text("connector", "sk", setup.own),
              text("connector", "en"),
              text("privacy", "sk"),
              text("privacy", "en"),
            ],
          })
        );
      }
      if (url.pathname === "/api/v1/organization/opendata/pages/preview") {
        const markdown = (body as { markdown: string }).markdown;
        return json({
          title: markdown.split("\n")[0].replace(/^# /, ""),
          html: `<p><strong>tučné</strong> <a href="javascript:alert(1)">zlý</a> <a href="https://ok.sk">dobrý</a></p><script>window.ran = true</script><img src=x onerror="window.ran = true">`,
        });
      }
      if (url.pathname.startsWith("/api/v1/organization/opendata/pages/")) {
        return setup.write ?? json(change("chg-0000abcd", "PendingApproval"), 202);
      }
      if (url.pathname === "/api/v1/projects/org/changes") {
        return json({
          apiVersion: "v1",
          kind: "ChangeList",
          items: setup.open ? [change("chg-11112222", "PendingApproval")] : [],
        });
      }
      if (url.pathname === "/api/v1/projects/org/changes/chg-11112222/manifests") {
        return json({
          items: [
            {
              apiVersion: "v1",
              kind: "Organization",
              metadata: { name: "bbsk" },
              spec: {
                appearance: {
                  pages: [
                    { page: "connector", lang: "sk", markdown: "# Náš konektor" },
                    { page: "connector", lang: "en", markdown: "# New" },
                  ],
                },
              },
            },
          ],
        });
      }
      if (url.pathname === "/api/v1/projects/org/changes/history") {
        return json({
          apiVersion: "v1",
          kind: "ChangeList",
          items: [change("chg-aaaa0001", "Merged"), change("chg-aaaa0002", "Merged")],
        });
      }
      if (url.pathname === "/api/v1/projects/org/changes/chg-aaaa0001") {
        return json({ ...change("chg-aaaa0001", "Merged"), planFields: [{ path: "spec.appearance.pages", to: [] }] });
      }
      if (url.pathname === "/api/v1/projects/org/changes/chg-aaaa0002") {
        return json({ ...change("chg-aaaa0002", "Merged"), planFields: [{ path: "spec.contacts", to: [] }] });
      }
      return undefined;
    },
  });
  return { ...result, requests };
}

const problem = (status: number, detail: string) =>
  new Response(JSON.stringify({ type: "about:blank", title: "refused", status, detail }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

async function openEditor(name = /Edit Konektor \(sk\)/) {
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name }));
  return user;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("open data pages", () => {
  it("lists_every_page_per_language_and_says_what_to_do_when_none_is_own", async () => {
    renderPages();
    const list = await screen.findByTestId("opendata-pages-list");
    expect(within(list).getAllByText("Built-in")).toHaveLength(4);
    expect(screen.getByText(/Every page shows the built-in text/)).toBeInTheDocument();
    // Only the Change that wrote spec.appearance.pages is page history.
    const history = await screen.findByTestId("opendata-pages-history");
    expect(within(history).getAllByRole("link")).toHaveLength(1);
    expect(within(history).getByRole("link").getAttribute("href")).toContain("chg-aaaa0001");
  });

  it("marks_the_text_an_open_change_would_change_as_waiting", async () => {
    renderPages({ open: true, own: "# Náš konektor" });
    const waiting = await screen.findByText("Waits for approval");
    expect(waiting.closest("li")?.textContent).toContain("en");
    expect(screen.getByText("Your text")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "chg-11112222" })).toBeInTheDocument();
  });

  it("previews_beside_the_editor_and_runs_nothing_the_answer_holds", async () => {
    const { container } = renderPages();
    await openEditor();
    const preview = await screen.findByText("tučné");
    expect(preview.tagName).toBe("STRONG");
    expect(screen.getByText("zlý").hasAttribute("href")).toBe(false);
    expect(screen.getByText("dobrý").getAttribute("rel")).toBe("noopener nofollow");
    expect(container.querySelector("script, img")).toBeNull();
    expect((window as { ran?: boolean }).ran).toBeUndefined();
    expect(screen.getByRole("textbox", { name: /Page text in Markdown/ })).toHaveValue(BUILT_IN);
    expect(screen.getByText(/e\.g\. "# Support"/)).toBeInTheDocument();
  });

  it("the_toolbar_writes_markdown_around_the_selection", async () => {
    renderPages();
    const user = await openEditor();
    const box = screen.getByRole("textbox", { name: /Page text in Markdown/ }) as HTMLTextAreaElement;
    box.setSelectionRange(BUILT_IN.indexOf("Vstavaný"), BUILT_IN.indexOf(" text."));
    await user.click(screen.getByRole("button", { name: "Bold" }));
    expect(box.value).toContain("**Vstavaný** text.");
  });

  it("publishes_as_a_change_that_waits_for_approval", async () => {
    const { requests } = renderPages();
    const user = await openEditor();
    const box = screen.getByRole("textbox", { name: /Page text in Markdown/ });
    fireEvent.change(box, { target: { value: "# Náš konektor\n\nNový text." } });
    await user.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText(/once an administrator approves the change/)).toBeInTheDocument();
    expect(screen.getByText("chg-0000abcd")).toBeInTheDocument();
    const put = requests.find((r) => r.method === "PUT");
    expect(put?.path).toBe("/api/v1/organization/opendata/pages/connector/sk");
    expect(put?.body).toEqual({ markdown: "# Náš konektor\n\nNový text." });
    await expectNoAxeViolations(document.body);
  });

  it("a_422_marks_the_field_with_the_servers_words", async () => {
    renderPages({ write: problem(422, "spec.appearance.pages[0].markdown: over 16 KiB") });
    const user = await openEditor();
    await user.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText(/over 16 KiB/)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /Page text in Markdown/ })).toHaveAttribute("aria-invalid", "true");
  });

  it("while_a_change_of_the_organization_is_open_publish_waits_and_says_why", async () => {
    renderPages({ open: true });
    await screen.findAllByText("Waits for approval");
    await openEditor(/Edit Konektor \(en\)/);
    const publish = screen.getByRole("button", { name: "Publish" });
    expect(publish).toBeDisabled();
    expect(document.getElementById(publish.getAttribute("aria-describedby") ?? "")?.textContent).toMatch(
      /A text for this page waits for approval/,
    );
  });

  it("a_text_over_the_limit_cannot_be_published_and_the_counter_says_why", async () => {
    renderPages();
    await openEditor();
    fireEvent.change(screen.getByRole("textbox", { name: /Page text in Markdown/ }), {
      target: { value: "é".repeat(8193) },
    });
    const publish = screen.getByRole("button", { name: "Publish" });
    expect(publish).toBeDisabled();
    expect(document.getElementById(publish.getAttribute("aria-describedby") ?? "")?.textContent).toBe(
      "16386 of 16384 bytes: 2 over the limit. Shorten the text to publish it.",
    );
  });

  it("a_409_says_to_decide_the_open_change_first", async () => {
    renderPages({ write: problem(409, "a Change on Organization bbsk is open") });
    const user = await openEditor();
    await user.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText(/Approve or reject it, then publish again/)).toBeInTheDocument();
  });

  it("a_403_on_a_write_and_on_the_list_says_whose_page_it_is", async () => {
    renderPages({ write: problem(403, "for the Organization's administrators") });
    const user = await openEditor();
    await user.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText(/You may not change the open data portal's pages/)).toBeInTheDocument();
    cleanup();
    renderPages(
      { list: problem(403, "editing the open data portal's pages is for the Organization's administrators") },
      true,
    );
    expect(await screen.findByText(/Only the Organization's administrators edit/)).toBeInTheDocument();
  });

  it("goes_back_to_the_built_in_text_after_a_confirmation", async () => {
    const { requests } = renderPages({ own: "# Náš konektor" });
    const user = await openEditor(/Edit Náš konektor \(sk\)/);
    await user.click(screen.getByRole("button", { name: "Use the built-in text" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Use the built-in text" }));
    await waitFor(() =>
      expect(requests.some((r) => r.method === "DELETE" && r.path.endsWith("/connector/sk"))).toBe(true),
    );
    expect(await screen.findByText("chg-0000abcd")).toBeInTheDocument();
  });
});

describe("page markdown", () => {
  it("wraps_and_prefixes", () => {
    expect(applyFormat("ab cd", 0, 2, "bold")).toEqual({ text: "**ab** cd", start: 2, end: 4 });
    expect(applyFormat("ab", 2, 2, "link").text).toBe("ab[link text](https://)");
    expect(applyFormat("a\nb", 0, 3, "code").text).toBe("```\na\nb\n```");
    expect(applyFormat("x\none\ntwo", 3, 7, "list").text).toBe("x\n- one\n- two");
    expect(applyFormat("## t", 1, 1, "heading").text).toBe("## t");
  });

  it("keeps_only_the_renderers_elements", () => {
    const { container } = render(
      <div>
        {previewElements(
          '<h2 onclick="x()">H</h2><iframe src="https://evil"></iframe><a href="/about">in</a><span style="color:red">s</span>',
        )}
      </div>,
    );
    expect(container.innerHTML).toBe('<div><p class="text-title font-semibold">H</p><a href="/about">in</a>s</div>');
  });
});
