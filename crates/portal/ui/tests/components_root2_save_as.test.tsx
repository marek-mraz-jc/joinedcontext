/**
 * Save as, the paths save_as.test.tsx leaves out: a space carried with what it holds, a space
 * reference given as an object, the server refusing a check or a proposal, a checked report that
 * renames and names what is needed, editing the name after a check, Cancel and Close, and the
 * row's own Save as button.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SaveAsDialog, SaveAsResourceAction } from "../src/components/SaveAsDialog";

vi.mock("../src/components/ChangeNotice", () => ({
  ChangeNotice: ({ change }: { change: { metadata: { name: string } } }) => <p>{change.metadata.name}</p>,
}));
vi.mock("../src/api/permissions", () => ({
  usePermissions: () => ({ can: () => true }),
}));

const API = "joinedcontext.com/v1alpha1";
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" } });
const listOf = (items: unknown[]) => ({ apiVersion: API, kind: "List", metadata: {}, items });
const space = { apiVersion: API, kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { urnSegment: "helsinki" } };
const inSpace = (name: string, ref: unknown) => ({ apiVersion: API, kind: "Endpoint", metadata: { name, namespace: "helsinki" }, spec: { contextSpaceRef: ref } });

let posts: { url: string; body: unknown }[];
let postAnswer: (url: URL) => Response;
let reads: Record<string, unknown>;

function mount(ui: React.ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{ui}</I18nextProvider>
    </QueryClientProvider>,
  );
}

const ENDPOINT = { project: "helsinki", kind: "Endpoint", plural: "endpoints", name: "bikes" };

describe("save as, further paths", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    posts = [];
    reads = { "/endpoints/bikes": inSpace("bikes", { name: "helsinki" }) };
    postAnswer = (url) =>
      url.searchParams.get("dryRun") === "All"
        ? json({ created: ["bikes-copy"], replaced: [], skipped: [], renamed: { bikes: "bikes-2" }, nativeFiles: 0, lane: "red", needs: [{ kind: "secret", where: "spec.auth" }] })
        : json({ apiVersion: API, kind: "Change", metadata: { name: "chg-7", namespace: "helsinki" }, status: { lane: "red", phase: "PendingApproval", plan: { create: 1 } } }, 202);
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        const url = new URL(request.url);
        if (request.method === "POST") {
          posts.push({ url: url.pathname + url.search, body: await request.clone().json().catch(() => null) });
          return postAnswer(url);
        }
        if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
        const key = Object.keys(reads).find((suffix) => url.pathname.endsWith(suffix));
        if (key) return json(reads[key]);
        return json(listOf([]));
      }),
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  it("a reference given as an object counts as a space; Check shows what was renamed and what is needed", async () => {
    const user = userEvent.setup();
    mount(<SaveAsDialog target={ENDPOINT} open onOpenChange={() => undefined} />);
    const name = await screen.findByLabelText(en.saveAs.name);
    await waitFor(() => expect(screen.getByRole("button", { name: en.saveAs.check })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: en.saveAs.check }));
    const report = await screen.findByRole("region", { name: en.saveAs.checked });
    expect(within(report).getByText("bikes → bikes-2")).toBeInTheDocument();
    expect(within(report).getByText("spec.auth")).toBeInTheDocument();
    expect(report).toHaveTextContent(en.import.report.need.secret);
    expect(posts).toHaveLength(1);

    // Editing the name withdraws the check: Propose is closed again until Check runs.
    await user.type(name, "x");
    expect(screen.queryByRole("region", { name: en.saveAs.checked })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.saveAs.propose })).toHaveAttribute("aria-disabled", "true");
  });

  it("says what the server refused on Check", async () => {
    const user = userEvent.setup();
    postAnswer = () => json({ title: "Unprocessable", status: 422, detail: "the name is taken" }, 422);
    mount(<SaveAsDialog target={ENDPOINT} open onOpenChange={() => undefined} />);
    await waitFor(() => expect(screen.getByRole("button", { name: en.saveAs.check })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: en.saveAs.check }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the name is taken");
  });

  it("says what the server refused on Propose after a green check", async () => {
    const user = userEvent.setup();
    let refuse = false;
    const green = postAnswer;
    postAnswer = (url) => (refuse ? json({ title: "Forbidden", status: 403, detail: "no right to propose there" }, 403) : green(url));
    mount(<SaveAsDialog target={ENDPOINT} open onOpenChange={() => undefined} />);
    await waitFor(() => expect(screen.getByRole("button", { name: en.saveAs.check })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: en.saveAs.check }));
    await screen.findByRole("region", { name: en.saveAs.checked });
    refuse = true;
    await user.click(screen.getByRole("button", { name: en.saveAs.propose }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no right to propose there");
  });

  it("a resource with no space reference offers no space choice", async () => {
    reads = { "/endpoints/bikes": inSpace("bikes", 7) };
    mount(<SaveAsDialog target={ENDPOINT} open onOpenChange={() => undefined} />);
    await waitFor(() => expect(screen.getByRole("button", { name: en.saveAs.check })).toBeEnabled());
    expect(screen.queryByRole("group", { name: /space/i })).not.toBeInTheDocument();
  });

  it("a space is sent with what it holds, and only what it holds", async () => {
    const user = userEvent.setup();
    reads = {
      "/spaces/helsinki": space,
      "/endpoints": listOf([inSpace("in-it", "helsinki"), inSpace("elsewhere", "tampere"), inSpace("object-ref", { name: "helsinki" })]),
    };
    mount(<SaveAsDialog target={{ project: "helsinki", kind: "ContextSpace", plural: "spaces", name: "helsinki" }} open onOpenChange={() => undefined} />);
    expect(await screen.findByText(en.saveAs.spaceNamed)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: en.saveAs.check })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: en.saveAs.check }));
    await screen.findByRole("region", { name: en.saveAs.checked });
    const sent = posts[0]?.body as { manifests: { kind: string; metadata: { name: string } }[]; conflictPolicy: string };
    expect(sent.manifests.map((m) => m.metadata.name)).toEqual(["helsinki", "in-it", "object-ref"]);
    expect(sent.conflictPolicy).toBe("rename");
  });

  it("Cancel closes and forgets the name typed; Close after a proposal closes too", async () => {
    const user = userEvent.setup();
    const closed = vi.fn();
    mount(<SaveAsDialog target={ENDPOINT} open onOpenChange={closed} />);
    const name = await screen.findByLabelText(en.saveAs.name);
    await user.clear(name);
    await user.type(name, "mine");
    await user.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(closed).toHaveBeenLastCalledWith(false);
    expect(screen.getByLabelText(en.saveAs.name)).toHaveValue("bikes-copy");

    await waitFor(() => expect(screen.getByRole("button", { name: en.saveAs.check })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: en.saveAs.check }));
    await screen.findByRole("region", { name: en.saveAs.checked });
    await user.click(screen.getByRole("button", { name: en.saveAs.propose }));
    expect(await screen.findByText("chg-7")).toBeInTheDocument();
    const dialog = screen.getByRole("dialog");
    await user.click(within(dialog).getAllByRole("button", { name: en.resourceDelete.close }).at(-1) as HTMLElement);
    expect(closed).toHaveBeenCalledTimes(2);
  });
});

describe("the row's Save as button", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
        if (url.pathname.endsWith("/endpoints/bikes")) return json(inSpace("bikes", "helsinki"));
        if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
        return json(listOf([]));
      }),
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  it("opens the dialog, tells the row, and closes it again", async () => {
    const user = userEvent.setup();
    const told = vi.fn();
    mount(<SaveAsResourceAction target={ENDPOINT} onOpenChange={told} />);
    await user.click(await screen.findByRole("button", { name: en.saveAs.action.replace("{name}", "bikes") }));
    expect(told).toHaveBeenLastCalledWith(true);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(told).toHaveBeenLastCalledWith(false);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });
});
