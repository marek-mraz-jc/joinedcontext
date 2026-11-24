// T-7706: the Scripts page typed edges: new script, bad tests JSON, limits, version, refused proposal and import.
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { odataValue } from "./odata";

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

const PROJECT = "helsinki";
const CODE = "const r = bento.v0_msg_as_structured();\nbento.v0_msg_set_structured(r);\n";
const TESTS = [{ name: "keeps", input: { e: 1 }, expected: { e: 1 } }];
const IDENTITY = { subject: "b7c1e0f4", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] };
const version = (v: string) => ({ version: v, state: "published", code: CODE, sha256: "0".repeat(64), tests: TESTS });
const SCRIPT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Script",
  metadata: { name: "sjtsk", namespace: PROJECT },
  spec: { language: "javascript", versions: [version("1.0.0"), version("1.1.0")] },
};
const pipeline = (name: string, pinned: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: PROJECT },
  spec: { steps: [{ kind: "script", scriptRef: { name: "sjtsk", version: pinned } }] },
});
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

interface Sent {
  method: string;
  path: string;
  body: Record<string, unknown>;
}

function renderPage({ refuseProposal = false, refuseImport = false, scripts = [SCRIPT] }: { refuseProposal?: boolean; refuseImport?: boolean; scripts?: unknown[] } = {}) {
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
        return json({ project: PROJECT, bootstrap: false, grants: [{ role: "pipeline-editor", binding: "editors", rule: { kinds: ["Script", "Pipeline"], verbs: ["propose"] } }] });
      }
      if (path.endsWith("JoinedContext.Portal.jc_script_test")) {
        const asked = JSON.parse(text) as { code: string };
        const erring = asked.code.includes("// ERRS") ? [{ name: "errs", ok: false, error: "boom", millis: 1 }] : [];
        return odataValue({ results: [{ name: "keeps", ok: true, output: { e: 1 }, millis: 3 }, ...erring] });
      }
      if (path.endsWith("/import")) {
        if (refuseImport) return json({ title: "Refused", detail: "the import is refused by a rule", status: 422 }, 422);
        return url.searchParams.get("dryRun") === "All"
          ? json({ created: [], replaced: ["Pipeline/a"], skipped: [], renamed: {}, native_files: 0, lane: "yellow", needs: [] })
          : json({ kind: "Change", metadata: { name: "chg-pins" }, spec: {}, status: { phase: "Pending" } }, 202);
      }
      if (request.method !== "GET" && path.includes("/scripts")) {
        if (refuseProposal) return json({ title: "Refused", detail: "the script is refused by a rule", status: 422 }, 422);
        return url.searchParams.get("dryRun") === "All"
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ kind: "Change", metadata: { name: "chg-script" }, spec: {}, status: { phase: "Pending" } });
      }
      if (path.endsWith("/scripts/sjtsk")) return json(SCRIPT);
      if (path.endsWith("/scripts")) return json(list(scripts));
      if (path.endsWith("/pipelines")) return json(list([pipeline("a", "1.0.0"), pipeline("b", "1.1.0")]));
      return json(list([]));
    }),
  );
  window.history.pushState({}, "", `/projects/${PROJECT}/scripts`);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return sent;
}

// The page remounts its workbench once while its lists settle: type into the box the page holds now.
async function replace(user: ReturnType<typeof userEvent.setup>, find: () => HTMLElement, text: string) {
  await new Promise((resolve) => setTimeout(resolve, 150));
  const box = find();
  await user.clear(box);
  await user.click(box);
  await user.paste(text);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  localStorage.clear();
  sessionStorage.clear();
});

describe("a new script", () => {
  it("names the script, refuses a bad name and a tests text that is not a list of tests", async () => {
    const user = userEvent.setup();
    renderPage({ scripts: [] });
    expect(await screen.findByText("No script yet: name one below, write its code and tests, then propose version 1.0.0.")).toBeInTheDocument();
    const name = await screen.findByRole("textbox", { name: /^Name/ });
    await user.type(name, "Bad Name");
    expect((await screen.findAllByText("Name the script with lower-case letters, digits and dashes.")).length).toBeGreaterThan(0);
    await user.clear(name);
    await user.type(name, "to-wgs84");
    const tests = () => screen.getByRole("textbox", { name: /^Tests/ });
    for (const text of ["not json", "{}", '[{"name":"x"}]', "[1]"]) {
      await replace(user, tests, text);
      expect((await screen.findAllByText("Write the tests as a JSON list of objects with name, input and expected.")).length).toBeGreaterThan(0);
      expect(screen.getByRole("button", { name: "Run tests" })).toHaveAttribute("aria-disabled", "true");
    }
    await replace(user, tests, JSON.stringify(TESTS));
    await waitFor(() => expect(screen.queryByText("Write the tests as a JSON list of objects with name, input and expected.")).not.toBeInTheDocument());
  });

  it("does nothing when a disabled Run or Propose button is clicked", async () => {
    const user = userEvent.setup();
    const sent = renderPage({ scripts: [] });
    await screen.findByText("No script yet: name one below, write its code and tests, then propose version 1.0.0.");
    await screen.findByRole("textbox", { name: /^Tests/ });
    const tests = () => screen.getByRole("textbox", { name: /^Tests/ });
    await replace(user, tests, "nope");
    expect((await screen.findAllByText("Write the tests as a JSON list of objects with name, input and expected.")).length).toBeGreaterThan(0);
    await user.click(screen.getByRole("button", { name: "Run tests" }));
    await user.click(screen.getByRole("button", { name: /^Propose version/ }));
    expect(sent.some((s) => s.path.includes("jc_script_test"))).toBe(false);
    expect(sent.some((s) => s.method !== "GET" && s.path.includes("/scripts"))).toBe(false);
  });

  it("refuses limits that are not whole numbers in range and sends the ones that are", async () => {
    const user = userEvent.setup();
    const sent = renderPage();
    await screen.findByRole("option", { name: "sjtsk" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
    await screen.findByRole("textbox", { name: /^Time per record/ });
    const timeout = () => screen.getByRole("textbox", { name: /^Time per record/ });
    const memory = () => screen.getByRole("textbox", { name: /^Memory per record/ });
    for (const bad of ["0", "201", "1.5", "abc"]) {
      await replace(user, timeout, bad);
      expect((await screen.findAllByText("Write a whole number from 1 to 200.")).length).toBeGreaterThan(0);
      expect(screen.getByRole("button", { name: "Run tests" })).toHaveAttribute("aria-disabled", "true");
    }
    await replace(user, timeout, "100");
    for (const bad of ["0", "65", "x"]) {
      await replace(user, memory, bad);
      expect((await screen.findAllByText("Write a whole number from 1 to 64.")).length).toBeGreaterThan(0);
    }
    await replace(user, memory, "16");
    await user.click(screen.getByRole("button", { name: "Run tests" }));
    await screen.findByText("Test results");
    const run = sent.find((s) => s.path.endsWith("JoinedContext.Portal.jc_script_test"));
    expect(run?.body).toMatchObject({ limits: { timeoutMs: 100, memoryMb: 16 } });
  });
});

describe("a stored script's results and proposal", () => {
  it("lists a passing and an erroring test, sorts them and blocks the proposal", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("option", { name: "sjtsk" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
    await screen.findByRole("textbox", { name: "Code" });
    await replace(user, () => screen.getByRole("textbox", { name: "Code" }), CODE + "// ERRS\n");
    await user.click(screen.getByRole("button", { name: "Run tests" }));
    expect(await screen.findByText("boom")).toBeInTheDocument();
    expect(screen.getAllByText("Pass").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Fail").length).toBeGreaterThan(0);
    for (const header of ["Test", "Result", "Output"]) {
      const button = screen.queryAllByRole("button", { name: new RegExp(`^${header}`) })[0];
      if (button) await user.click(button);
    }
    expect(screen.getAllByText("1 test fails: fix the code or the test.").length).toBeGreaterThan(0);
  });

  it("keeps a typed version and says what is wrong with it", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("option", { name: "sjtsk" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
    await screen.findByRole("textbox", { name: "Code" });
    await replace(user, () => screen.getByRole("textbox", { name: "Code" }), CODE + "// more\n");
    await user.click(screen.getByRole("button", { name: "Run tests" }));
    await screen.findByText("Test results");
    const versionBox = () => screen.getByRole("textbox", { name: /^Version/ });
    await replace(user, versionBox, "1.1.0");
    expect((await screen.findAllByText("Pick a version above 1.1.0.")).length).toBeGreaterThan(0);
    await replace(user, versionBox, "banana");
    expect((await screen.findAllByText("Write the version as MAJOR.MINOR.PATCH, e.g. 1.1.0.")).length).toBeGreaterThan(0);
  });

  it("shows the refusal when the proposal is refused", async () => {
    const user = userEvent.setup();
    const sent = renderPage({ refuseProposal: true });
    await screen.findByRole("option", { name: "sjtsk" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
    await screen.findByRole("textbox", { name: "Code" });
    await replace(user, () => screen.getByRole("textbox", { name: "Code" }), CODE + "// more\n");
    await user.click(screen.getByRole("button", { name: "Run tests" }));
    await screen.findByText("Test results");
    await user.click(screen.getByRole("button", { name: /^Propose version/ }));
    expect(await screen.findByText("the script is refused by a rule")).toBeInTheDocument();
    expect(sent.filter((s) => s.method !== "GET" && s.path.includes("/scripts")).length).toBeGreaterThan(0);
    expect(screen.queryByText(/is proposed\./)).not.toBeInTheDocument();
  });
});

describe("the pipelines that pin a script", () => {
  it("ticks and unticks a pipeline, picks another target and says why a refused import moved nothing", async () => {
    const user = userEvent.setup();
    const sent = renderPage({ refuseImport: true });
    await screen.findByRole("option", { name: "sjtsk" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Script" }), "sjtsk");
    const used = await screen.findByRole("heading", { name: "Pipelines using sjtsk" });
    const card = used.closest("section, [aria-labelledby]") as HTMLElement;
    const box = within(card).getAllByRole("checkbox", { name: "a" })[0];
    await user.click(box);
    expect(box).toBeChecked();
    await user.click(box);
    expect(box).not.toBeChecked();
    expect(within(card).getAllByText("On the target").length).toBeGreaterThan(0);
    await user.selectOptions(within(card).getByRole("combobox", { name: "Move pins to" }), "1.0.0");
    await user.click(within(card).getAllByRole("checkbox", { name: "b" })[0]);
    for (const header of ["Pipeline", "Pinned version"]) {
      const button = within(card).queryAllByRole("button", { name: new RegExp(`^${header}`) })[0];
      if (button) await user.click(button);
    }
    await user.click(within(card).getByRole("button", { name: "Move 1 pipeline to 1.0.0" }));
    expect(await within(card).findByText("the import is refused by a rule")).toBeInTheDocument();
    expect(sent.filter((s) => s.path.includes("/import")).length).toBeGreaterThan(0);
  });
});
