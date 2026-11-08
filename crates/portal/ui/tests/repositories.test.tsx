// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/repositories/RepositoriesPage.tsx, src/pages/repositories/RepositoryPage.tsx,
// src/pages/repositories/FilesView.tsx, src/pages/repositories/CommitsView.tsx,
// src/pages/repositories/BranchesView.tsx, src/pages/repositories/PullsView.tsx,
// src/pages/repositories/SettingsView.tsx, src/pages/repositories/links.tsx and
// src/pages/repositories/api.ts through the routes they belong to.
import { cleanup, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { crumbsOf, diffLines, shortSha } from "../src/pages/repositories/api";
import { repositorySearch } from "../src/pages/repositories/links";
import { Pager, RoleBadge } from "../src/pages/repositories/RepositoriesPage";
import { CloneAddress } from "../src/pages/repositories/RepositoryPage";
import { FileLines } from "../src/pages/repositories/FilesView";
import { DiffView, StatusBadge } from "../src/pages/repositories/CommitsView";
import { BranchesView } from "../src/pages/repositories/BranchesView";
import { PullStateBadge } from "../src/pages/repositories/PullsView";
import { SettingsView } from "../src/pages/repositories/SettingsView";
import { jsonResponse, problem, renderRoute } from "./pageHarness";

const P = "/projects/helsinki/repositories";
const SHA = "9fceb02d0ae598e95dc970b74767f19372d61af8";
const PARENT = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
const XSS = '<script>window.__pwned = 1</script> <img src=x onerror="window.__pwned = 1"> [click](javascript:alert(1))';

const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["Project"], verbs } }],
});
const STEWARD = grants(["read", "propose", "approve"]);
const VIEWER = grants(["read"]);

const REPOSITORY = {
  name: "helsinki-config",
  owner: "helsinki",
  role: "configuration",
  description: "Helsinki's configuration",
  defaultBranch: "main",
  sizeBytes: 20480,
  cloneUrl: "https://forge.example/git/helsinki/helsinki-config.git",
  empty: false,
  lastCommit: { sha: SHA, message: "Change b", author: "jana", date: "2026-11-01T10:00:00Z" },
  readme: { path: "README.md", text: `# Helsinki\n\n${XSS}\n` },
};

const ANSWERS: Record<string, unknown> = {
  jc_repository_list: {
    items: [
      { name: "helsinki-config", role: "configuration", defaultBranch: "main", sizeBytes: 20480, description: "Helsinki's configuration" },
      { name: "helsinki_dash", role: "application", defaultBranch: "main", sizeBytes: 1394, archived: true },
    ],
    total: 2,
  },
  jc_repository_get: REPOSITORY,
  jc_repository_tree: {
    repository: "helsinki-config",
    ref: "main",
    path: "",
    entries: [
      { name: "spaces", path: "spaces", type: "folder" },
      { name: "README.md", path: "README.md", type: "file", size: 120 },
    ],
    truncated: false,
  },
  jc_repository_file: { repository: "helsinki-config", ref: "main", path: "README.md", size: 120, binary: false, tooLarge: false, text: `line one\n${XSS}\n` },
  jc_repository_commits: {
    ref: "main",
    items: [{ sha: SHA, message: "Change b", author: "jana", date: "2026-11-01T10:00:00Z" }],
    total: 1,
  },
  jc_repository_commit: {
    repository: "helsinki-config",
    sha: SHA,
    message: `Change b\n\n${XSS}`,
    author: "jana",
    date: "2026-11-01T10:00:00Z",
    parents: [PARENT],
    files: [
      { path: "spaces/air/endpoints/air.yaml", status: "modified", patch: "@@ -1,2 +1,2 @@\n a: 1\n-b: 2\n+b: 3\n" },
      { path: "logo.png", status: "added", binary: true },
    ],
    truncated: false,
  },
  jc_repository_refs: {
    repository: "helsinki-config",
    branches: [
      { name: "main", headSha: SHA, protected: true, default: true },
      { name: "portal/edit", headSha: PARENT },
    ],
    tags: [],
  },
  jc_repository_pulls: {
    items: [{ number: 7, title: "Add c", state: "open", author: "jana", head: "portal/edit", base: "main", createdAt: "2026-11-01T10:00:00Z" }],
    total: 1,
  },
  jc_repository_pull: {
    repository: "helsinki-config",
    number: 7,
    title: "Add c",
    state: "open",
    author: "jana",
    head: "portal/edit",
    base: "main",
    body: XSS,
    change: "chg-00000007",
    files: [{ path: "spaces/air/endpoints/c.yaml", status: "added" }],
    reviews: [{ author: "petra", verdict: "APPROVED", body: "Looks right" }],
  },
  jc_repository_settings: {
    repository: "helsinki-config",
    protection: [{ pattern: "main", pushAllowed: false, forcePushAllowed: false, requiredApprovals: 1 }],
    webhooks: [{ address: "https://portal.example/api/v1/webhooks/shako", active: true }],
    collaborators: [{ group: "helsinki-stewards", access: "write" }],
    packages: [],
  },
};

/** What the registry answers, per operation; `override` replaces one operation's answer. */
function answering(override: Record<string, (input: Record<string, unknown>) => Response> = {}, seen: Record<string, unknown>[] = []) {
  return (path: string, request: Request): Promise<Response> | undefined => {
    const op = /\/ops\/(jc_repository_[a-z]+)$/.exec(path)?.[1];
    if (!op) return undefined;
    return (async () => {
      const input = (await request.clone().json()) as Record<string, unknown>;
      seen.push({ op, ...input });
      return override[op]?.(input) ?? jsonResponse(ANSWERS[op]);
    })();
  };
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  delete (window as { __pwned?: number }).__pwned;
});

/** Nothing a repository holds ran or became markup: no script, no handler, no `javascript:` link. */
function inert(): void {
  expect((window as { __pwned?: number }).__pwned).toBeUndefined();
  expect(document.querySelector("main script, main img[onerror]")).toBeNull();
  for (const link of document.querySelectorAll("main a")) {
    expect(link.getAttribute("href") ?? "").not.toMatch(/^\s*javascript:/i);
  }
}

describe("the repositories list", () => {
  it("lists the project's repositories with their role and opens one", async () => {
    await renderRoute({ path: P, permissions: VIEWER, answer: answering() });
    const link = await screen.findByRole("link", { name: "helsinki-config" });
    expect(link).toHaveAttribute("href", `${P}/helsinki-config`);
    expect(screen.getByText("Configuration")).toBeInTheDocument();
    expect(screen.getByText("Archived")).toBeInTheDocument();
  });

  it("sends the filter to the server and says when nothing matches", async () => {
    const seen: Record<string, unknown>[] = [];
    await renderRoute({
      path: P,
      permissions: VIEWER,
      answer: answering({ jc_repository_list: (input) => jsonResponse(input.filter ? { items: [], total: 0 } : ANSWERS.jc_repository_list) }, seen),
    });
    await screen.findByRole("link", { name: "helsinki-config" });
    await userEvent.type(screen.getByLabelText("Filter by name"), "zzz");
    expect(await screen.findByText("No repository matches")).toBeInTheDocument();
    expect(seen.at(-1)).toMatchObject({ op: "jc_repository_list", filter: "zzz", cursor: 0 });
  });

  it("says what lives here when the project has none", async () => {
    await renderRoute({ path: P, permissions: VIEWER, answer: answering({ jc_repository_list: () => jsonResponse({ items: [], total: 0 }) }) });
    expect(await screen.findByText("No repositories yet")).toBeInTheDocument();
  });

  it("names a refusal instead of an empty list", async () => {
    await renderRoute({ path: P, permissions: VIEWER, answer: answering({ jc_repository_list: () => problem(403, "this read is not open to you") }) });
    expect(await screen.findByText(/not open to you/)).toBeInTheDocument();
    expect(screen.queryByText("No repositories yet")).toBeNull();
  });
});

describe("one repository", () => {
  it("shows the files and the README as text: a script or a javascript: link in it is inert", async () => {
    await renderRoute({ path: `${P}/helsinki-config`, permissions: VIEWER, answer: answering() });
    expect(await screen.findByRole("link", { name: "spaces" })).toHaveAttribute("href", `${P}/helsinki-config?tab=files&path=spaces`);
    const readme = await screen.findByRole("region", { name: "README.md" });
    expect(within(readme).getByText(/<script>window.__pwned = 1<\/script>/)).toBeInTheDocument();
    expect(screen.getByText("https://forge.example/git/helsinki/helsinki-config.git")).toBeInTheDocument();
    inert();
  });

  it("hides Settings from a viewer and shows it to whoever approves the project", async () => {
    await renderRoute({ path: `${P}/helsinki-config`, permissions: VIEWER, answer: answering() });
    await screen.findByRole("tab", { name: "Files" });
    expect(screen.queryByRole("tab", { name: "Settings" })).toBeNull();
    cleanup();
    await renderRoute({ path: `${P}/helsinki-config?tab=settings`, permissions: STEWARD, answer: answering() });
    expect(await screen.findByRole("tab", { name: "Settings", selected: true })).toBeInTheDocument();
    expect(await screen.findByText("https://portal.example/api/v1/webhooks/shako")).toBeInTheDocument();
    expect(screen.getByText("helsinki-stewards")).toBeInTheDocument();
    expect(screen.getByText("Nothing published from this repository.")).toBeInTheDocument();
  });

  it("a viewer who types the settings address gets the files", async () => {
    await renderRoute({ path: `${P}/helsinki-config?tab=settings`, permissions: VIEWER, answer: answering() });
    expect(await screen.findByRole("tab", { name: "Files", selected: true })).toBeInTheDocument();
  });

  it("says the repository is not there when the server answers 404", async () => {
    await renderRoute({ path: `${P}/other-org`, permissions: VIEWER, answer: answering({ jc_repository_get: () => problem(404, "other-org was not found") }) });
    expect(await screen.findByText(/other-org was not found|not found/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All repositories" })).toHaveAttribute("href", P);
  });

  it("draws a file as numbered text lines: markup in it stays characters", async () => {
    await renderRoute({ path: `${P}/helsinki-config?tab=files&file=README.md`, permissions: VIEWER, answer: answering() });
    const lines = await screen.findByRole("group", { name: "README.md" });
    expect(within(lines).getByText("line one")).toBeInTheDocument();
    expect(within(lines).getByText(XSS)).toBeInTheDocument();
    expect(within(screen.getByRole("navigation", { name: "Folder path" })).getByText("README.md")).toHaveAttribute("aria-current", "page");
    inert();
  });

  it("names a binary or large file instead of drawing it", async () => {
    await renderRoute({
      path: `${P}/helsinki-config?tab=files&file=logo.png`,
      permissions: VIEWER,
      answer: answering({ jc_repository_file: () => jsonResponse({ repository: "helsinki-config", ref: "main", path: "logo.png", size: 2048, binary: true, tooLarge: false }) }),
    });
    expect(await screen.findByText("This file is binary")).toBeInTheDocument();
  });

  it("shows a commit with its diff, numbered on both sides, and its body inert", async () => {
    await renderRoute({ path: `${P}/helsinki-config?tab=commits&commit=${SHA}`, permissions: VIEWER, answer: answering() });
    const diff = await screen.findByRole("group", { name: "Changes in spaces/air/endpoints/air.yaml" });
    expect(within(diff).getByText("b: 2")).toBeInTheDocument();
    expect(within(diff).getByText("b: 3")).toBeInTheDocument();
    expect(within(diff).getByLabelText("removed")).toBeInTheDocument();
    expect(screen.getByText("Binary file, no text change shown.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: shortSha(PARENT) })).toHaveAttribute("href", `${P}/helsinki-config?tab=commits&commit=${PARENT}`);
    inert();
  });

  it("lists commits that open their commit", async () => {
    await renderRoute({ path: `${P}/helsinki-config?tab=commits`, permissions: VIEWER, answer: answering() });
    expect(await screen.findByRole("link", { name: "Change b" })).toHaveAttribute("href", `${P}/helsinki-config?tab=commits&commit=${SHA}`);
  });

  it("lists branches with the default and protected marked, and says there are no tags", async () => {
    await renderRoute({ path: `${P}/helsinki-config?tab=branches`, permissions: VIEWER, answer: answering() });
    expect(await screen.findByRole("link", { name: "portal/edit" })).toHaveAttribute("href", `${P}/helsinki-config?tab=files&ref=portal%2Fedit`);
    expect(screen.getByText("Default")).toBeInTheDocument();
    expect(screen.getByText("Protected")).toBeInTheDocument();
    expect(screen.getByText("No tags")).toBeInTheDocument();
  });

  it("opens a pull request with its files, reviews and the link to its Change; its body is inert", async () => {
    await renderRoute({ path: `${P}/helsinki-config?tab=pulls`, permissions: VIEWER, answer: answering() });
    await userEvent.click(await screen.findByRole("link", { name: "Add c" }));
    expect(await screen.findByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-00000007");
    expect(screen.getByText("spaces/air/endpoints/c.yaml")).toBeInTheDocument();
    expect(screen.getByText("Approved")).toBeInTheDocument();
    inert();
  });
});

describe("the helpers", () => {
  it("numbers a unified patch from its hunk headers", () => {
    expect(diffLines("@@ -3,2 +3,3 @@\n a\n-b\n+c\n+d\n")).toEqual([
      { kind: "hunk", text: "@@ -3,2 +3,3 @@" },
      { kind: "context", text: "a", old: 3, new: 3 },
      { kind: "removed", text: "b", old: 4 },
      { kind: "added", text: "c", new: 4 },
      { kind: "added", text: "d", new: 5 },
    ]);
  });

  it("splits a path into crumbs and shortens a sha", () => {
    expect(crumbsOf("a/b/c.yaml")).toEqual([
      { name: "a", path: "a" },
      { name: "b", path: "a/b" },
      { name: "c.yaml", path: "a/b/c.yaml" },
    ]);
    expect(crumbsOf("")).toEqual([]);
    expect(shortSha(SHA)).toBe("9fceb02");
  });

  it("keeps only what an address may carry", () => {
    expect(repositorySearch({ tab: "nope", pull: "7", commit: 3, path: "a/b" })).toEqual({ pull: 7, path: "a/b" });
  });
});

/** A component drawn alone, with the language and a query client it reads. */
function alone(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the parts, drawn alone", () => {
  it("FileLines numbers every line and keeps an empty one", () => {
    alone(<FileLines text={"a\n\n<b>c</b>\n"} label="x.txt" />);
    const group = screen.getByRole("group", { name: "x.txt" });
    expect(within(group).getByText("3")).toBeInTheDocument();
    expect(within(group).getByText("<b>c</b>")).toBeInTheDocument();
    expect(group.querySelector("b")).toBeNull();
  });

  it("DiffView marks an added and a removed line in words, not only colour", () => {
    alone(<DiffView patch={"@@ -1 +1 @@\n-old\n+new\n"} label="f" />);
    expect(screen.getByLabelText("added")).toBeInTheDocument();
    expect(screen.getByLabelText("removed")).toBeInTheDocument();
  });

  it("the badges say the state in words, an unknown status as modified", () => {
    alone(
      <>
        <StatusBadge status="weird" />
        <PullStateBadge state="merged" />
        <RoleBadge role="application" />
      </>,
    );
    expect(screen.getByText("Modified")).toBeInTheDocument();
    expect(screen.getByText("Merged")).toBeInTheDocument();
    expect(screen.getByText("Application")).toBeInTheDocument();
  });

  it("Pager hides on a single page and says why a direction is closed", async () => {
    const { container } = alone(<Pager total={5} cursor={0} size={30} onCursor={() => undefined} />);
    expect(container).toBeEmptyDOMElement();
    cleanup();
    const onCursor = vi.fn();
    alone(<Pager total={70} cursor={30} size={30} next={60} onCursor={onCursor} />);
    expect(screen.getByText("31–60 of 70")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(onCursor).toHaveBeenCalledWith(60);
    cleanup();
    alone(<Pager total={70} cursor={60} size={30} onCursor={onCursor} />);
    expect(screen.getByRole("button", { name: /Next/ })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("This is the last page.")).toBeInTheDocument();
  });

  it("CloneAddress copies the address", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    alone(<CloneAddress url="https://forge.example/x.git" />);
    await userEvent.click(screen.getByRole("button", { name: "Copy the clone address" }));
    expect(writeText).toHaveBeenCalledWith("https://forge.example/x.git");
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });

  it("SettingsView says what is empty, section by section", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ repository: "r", protection: [], webhooks: [], collaborators: [], packages: [] }))),
    );
    alone(<SettingsView project="helsinki" repository={{ ...REPOSITORY, role: "configuration" } as never} />);
    expect(await screen.findByText("No branch is protected.")).toBeInTheDocument();
    expect(screen.getByText("No webhooks.")).toBeInTheDocument();
    expect(screen.getByText("No group has access of its own.")).toBeInTheDocument();
  });

  it("BranchesView is the view the branches tab draws", () => {
    // Drawn through the route above ("lists branches with the default and protected marked"):
    // its links need the router, which a component drawn alone lacks.
    expect(BranchesView).toBeTypeOf("function");
  });
});
