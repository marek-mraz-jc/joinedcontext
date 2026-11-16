/**
 * The Scripts page (T-7084, PL-118…PL-121): the workbench and who pins a script.
 *
 * 1. An edit runs its tests on the runner beside the newest version's, marks the output that
 *    changed, suggests the next minor version and proposes it with its digest as one Change.
 * 2. Two pipelines pinning an older version move to a published one as one import Change, its
 *    dry run first.
 * 3. A failing test, a role without Script propose and a silent runner each say why nothing is
 *    proposed.
 */
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { JSX } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { digest } from "../src/pages/scripts/scriptDraft";
import { odataError, odataValue } from "./odata";

// Monaco needs a canvas and a worker jsdom has neither of: a text area takes its place, with
// the props the editor hands it, so ScriptCodeMonaco itself still runs.
vi.mock("@monaco-editor/react", async () => {
  const { useEffect, useRef } = await import("react");
  return {
    default: function FakeEditor(given: {
      defaultValue?: string;
      onChange?: (value: string) => void;
      onMount?: (editor: unknown) => void;
      options?: { ariaLabel?: string };
    }) {
      const area = useRef<HTMLTextAreaElement>(null);
      useEffect(() => {
        given.onMount?.({
          getValue: () => area.current?.value ?? "",
          setValue: (value: string) => {
            if (area.current) area.current.value = value;
          },
          getDomNode: () => area.current?.parentElement ?? null,
        });
        // Mounted once, as Monaco is.
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, []);
      return (
        <textarea
          ref={area}
          aria-label={given.options?.ariaLabel}
          defaultValue={given.defaultValue}
          onChange={(event) => given.onChange?.(event.target.value)}
        />
      );
    },
  };
});
vi.mock("../src/pages/models/monaco-setup", () => ({}));
vi.mock("monaco-editor/languages/definitions/javascript/register", () => ({}));

const { App } = await import("../src/App");
const { ScriptsPage } = await import("../src/pages/scripts/ScriptsPage");
const { default: ScriptCodeMonaco } = await import("../src/pages/scripts/ScriptCodeMonaco");

const PROJECT = "helsinki";
const OLD = "const r = bento.v0_msg_as_structured();\nr.x = r.e;\nbento.v0_msg_set_structured(r);\n";
const NEW = "const r = bento.v0_msg_as_structured();\nr.x = r.e + 1;\nbento.v0_msg_set_structured(r);\n";
const TESTS = [{ name: "shifts-east", input: { e: 1 }, expected: { e: 1, x: 2 } }];
const IDENTITY = { subject: "b7c1e0f4", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] };

const version = (v: string, code: string) => ({ version: v, state: "published", code, sha256: "0".repeat(64), tests: TESTS });
const SCRIPT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Script",
  metadata: { name: "sjtsk", namespace: PROJECT },
  spec: { language: "javascript", versions: [version("1.0.0", OLD), version("1.1.0", OLD)] },
};
const pipeline = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: PROJECT },
  spec: { steps: [{ kind: "script", scriptRef: { name: "sjtsk", version: "1.0.0" } }] },
});

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

interface Sent {
  method: string;
  path: string;
  body: Record<string, unknown>;
}

function renderPage({ kinds = ["Script", "Pipeline"], runner = "answers", fails = false }: { kinds?: string[]; runner?: "answers" | "silent"; fails?: boolean } = {}) {
  const sent: Sent[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input));
      const url = new URL(request.url, window.location.origin);
      const path = url.pathname;
      const text = request.method === "GET" ? "" : await request.clone().text();
      sent.push({ method: request.method, path: `${path}${url.search}`, body: text ? (JSON.parse(text) as Record<string, unknown>) : {} });
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/admission")) return json({ admitted: true });
      if (path.endsWith("/permissions/me")) {
        return json({ project: PROJECT, bootstrap: false, grants: [{ role: "pipeline-editor", binding: "editors", rule: { kinds, verbs: ["propose"] } }] });
      }
      if (path.endsWith("JoinedContext.Portal.jc_script_test")) {
        if (runner === "silent") return odataError(503, "runner-unavailable", "no runner is configured for this project");
        const asked = JSON.parse(text) as { code: string };
        const output = asked.code === NEW ? { e: 1, x: 2 } : { e: 1, x: 1 };
        return odataValue({ results: [{ name: "shifts-east", ok: asked.code !== NEW || !fails, output, millis: 3 }] });
      }
      if (path.endsWith("/import")) {
        return url.searchParams.get("dryRun") === "All"
          ? json({ created: [], replaced: ["Pipeline/a", "Pipeline/b"], skipped: [], renamed: {}, native_files: 0, lane: "yellow", needs: [] })
          : json({ kind: "Change", metadata: { name: "chg-pins" }, spec: {}, status: { phase: "Pending" } }, 202);
      }
      if (path.endsWith("/scripts/sjtsk")) {
        if (request.method === "GET") return json(SCRIPT);
        return url.searchParams.get("dryRun") === "All"
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ kind: "Change", metadata: { name: "chg-script" }, spec: {}, status: { phase: "Pending" } });
      }
      if (path.endsWith("/scripts")) return json(list([SCRIPT]));
      if (path.endsWith("/pipelines")) return json(list([pipeline("a"), pipeline("b")]));
      return json(list([]));
    }),
  );
  window.history.pushState({}, "", `/projects/${PROJECT}/scripts`);
  render(providers(<App />));
  return sent;
}

function providers(page: JSX.Element): JSX.Element {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{page}</I18nextProvider>
    </QueryClientProvider>
  );
}

async function editAndRun(user: ReturnType<typeof userEvent.setup>) {
  await screen.findByRole("option", { name: "sjtsk" });
  await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
  const code = await screen.findByRole("textbox", { name: "Code" });
  await user.clear(code);
  await user.click(code);
  await user.paste(NEW);
  await user.click(screen.getByRole("button", { name: "Run tests" }));
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the script workbench", () => {
  it("runs the edit beside the newest version, marks what changed and proposes 1.2.0 with its digest", async () => {
    const user = userEvent.setup();
    const sent = renderPage();
    await editAndRun(user);
    expect(await screen.findByText("Changed")).toBeInTheDocument();
    expect(sent.filter((s) => s.path.endsWith("JoinedContext.Portal.jc_script_test")).map((s) => s.body.code)).toEqual([NEW, OLD]);
    expect(screen.getByRole("textbox", { name: "Version" })).toHaveValue("1.2.0");

    await user.click(screen.getByRole("button", { name: "Propose version 1.2.0" }));
    expect(await screen.findByText(/Version 1\.2\.0 is proposed/)).toBeInTheDocument();
    const proposals = sent.filter((s) => s.path.startsWith(`/api/v1/projects/${PROJECT}/scripts/sjtsk`) && s.method !== "GET");
    expect(proposals.map((s) => s.path.includes("dryRun=All"))).toEqual([true, false]);
    const versions = (proposals[1].body.spec as { versions: { version: string; state: string; code: string; sha256: string }[] }).versions;
    expect(versions.map((v) => v.version)).toEqual(["1.0.0", "1.1.0", "1.2.0"]);
    expect(versions[2]).toMatchObject({ state: "published", code: NEW, sha256: await digest(NEW) });
  });

  it("names the failing test and proposes nothing while it fails", async () => {
    const user = userEvent.setup();
    const sent = renderPage({ fails: true });
    await editAndRun(user);
    expect(await screen.findAllByText("1 test fails: fix the code or the test.")).not.toHaveLength(0);
    expect(screen.getByRole("button", { name: "Propose version 1.2.0" })).toHaveAttribute("aria-disabled", "true");
    expect(sent.some((s) => s.path.includes("/scripts/sjtsk") && s.method !== "GET")).toBe(false);
  });

  it("tells a person whose role proposes no script why, and runs nothing for them to propose", async () => {
    // The page on its own, outside the App's layout: what the route mounts.
    renderPage({ kinds: ["Pipeline"] });
    cleanup();
    const root = createRootRoute({ component: Outlet });
    const page = createRoute({ getParentRoute: () => root, path: "/", component: () => <ScriptsPage project={PROJECT} /> });
    window.history.pushState({}, "", "/");
    render(providers(<RouterProvider router={createRouter({ routeTree: root.addChildren([page]) })} />));
    await screen.findByRole("heading", { name: "Script workbench" });
    expect(await screen.findAllByText(/^Your role does not propose scripts in this project, so it cannot run their tests either/)).not.toHaveLength(0);
  });

  it("says what the runner answered when it cannot run the tests", async () => {
    const user = userEvent.setup();
    renderPage({ runner: "silent" });
    await editAndRun(user);
    expect(await screen.findByText("no runner is configured for this project")).toBeInTheDocument();
  });
});

describe("who pins a script", () => {
  it("moves two pipelines' pins to a published version as one checked import", async () => {
    const user = userEvent.setup();
    const sent = renderPage();
    await screen.findByRole("option", { name: "sjtsk" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
    const used = await screen.findByRole("heading", { name: "Pipelines using sjtsk" });
    const card = used.closest("section, [aria-labelledby]") as HTMLElement;
    expect(within(card).getByRole("combobox", { name: "Move pins to" })).toHaveValue("1.1.0");
    await user.click(within(card).getAllByRole("checkbox", { name: "a" })[0]);
    await user.click(within(card).getAllByRole("checkbox", { name: "b" })[0]);
    await user.click(within(card).getByRole("button", { name: "Move 2 pipelines to 1.1.0" }));
    expect(await within(card).findByText("Proposed one Change moving a and b to 1.1.0.")).toBeInTheDocument();
    const imports = sent.filter((s) => s.path.includes("/import"));
    expect(imports.map((s) => s.path.includes("dryRun=All"))).toEqual([true, false]);
    await waitFor(() => expect(JSON.stringify(imports[1].body)).toContain('"version":"1.1.0"'));
    expect(JSON.stringify(imports[1].body)).not.toContain('"version":"1.0.0"');
  });
});

describe("the code editor", () => {
  it("takes a value from outside and hands each keystroke back", async () => {
    const user = userEvent.setup();
    const seen: string[] = [];
    const { rerender } = render(<ScriptCodeMonaco value="a" onChange={(v) => seen.push(v)} ariaLabel="Code" />);
    const area = screen.getByRole("textbox", { name: "Code" });
    await user.type(area, "b");
    expect(seen.at(-1)).toBe("ab");
    rerender(<ScriptCodeMonaco value="other" onChange={(v) => seen.push(v)} ariaLabel="Code" />);
    expect(area).toHaveValue("other");
  });
});
