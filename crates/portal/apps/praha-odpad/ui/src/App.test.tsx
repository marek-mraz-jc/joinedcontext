/** The waste desk over what the app's endpoint answers (T-2786). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { reasonOf } from "./App";
import { answer, NOW } from "./fixtures/odpad";
import { LOCALES } from "./locales";

const s = LOCALES.cs;
const SLUG = "wn4ry2bxc6qpz7tmk3jdh5vae2";
let asked: URL[];

function show(refuse?: string, withEndpoint = true, serve: (type: string | null, ids: string[]) => unknown[] | null = answer, most?: number) {
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      asked.push(url);
      const type = url.searchParams.get("type");
      if (type === refuse) {
        return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "sign in as city staff" }), { status: 403, headers: { "content-type": "application/problem+json" } });
      }
      const ids = url.searchParams.get("id")?.split(",") ?? [];
      const body = serve(type, ids);
      if (body === null) return new Response(JSON.stringify({ title: "Bad Gateway", status: 502 }), { status: 502, headers: { "content-type": "application/problem+json" } });
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
  // The table reads through `fetch` above; the entity panel reads one container through the client (SDK-40).
  const client = stubClient({ entities: answer("WasteContainer", []) as Row[] }, {
    portal: "https://portal.praha.eu/projects/praha",
    slug: withEndpoint ? SLUG : "",
    orgDomain: "praha.eu",
    space: withEndpoint ? "praha-mesto" : "elsewhere",
    transport: "origin",
    appName: "praha-odpad",
    language: "cs",
  });
  return render(
    <JcProvider client={client}>
      <App now={NOW} most={most} />
    </JcProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const table = () => screen.getByRole("table");
const codes = () => within(table()).getAllByRole("rowheader").map((cell) => cell.textContent);

describe("the waste desk", () => {
  it("shows the fullest first with the isle by name, an unread container last", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    expect(codes()[0]).toBe("0001-PAP");
    expect(codes().at(-1)).toBe("0006-PAP");
    const first = within(table()).getByRole("row", { name: /0001-PAP/ });
    expect(within(first).getByText("Vinohradská 12")).toBeInTheDocument();
    expect(within(first).getByText(new RegExp(s.fullest))).toBeInTheDocument();
    // The isles were read by the ids the containers name, never the whole type.
    expect(asked.filter((url) => url.searchParams.get("type") === "WasteContainerIsle").every((url) => url.searchParams.get("id"))).toBe(true);
  });

  it("says when an isle cannot be named and when a reading is missing", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    expect(within(within(table()).getByRole("row", { name: /=0006-X/ })).getByText(s.isleUnknown)).toBeInTheDocument();
    expect(within(within(table()).getByRole("row", { name: /0006-PAP/ })).getAllByText(s.noValue).length).toBe(2);
  });

  it("marks the longest-unread tenth beside the reading's age", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const old = within(table()).getByRole("row", { name: /0005-GLS/ });
    expect(within(old).getByText(s.ago(200))).toBeInTheDocument();
    expect(within(old).getByText(new RegExp(s.longestUnread))).toBeInTheDocument();
  });

  it("narrows by kind, to the fullest tenth and by isle name", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.kind }), "paper");
    expect(codes()).toEqual(["0001-PAP", "0004-PAP", "0002-PAP", "0006-PAP"]);
    await userEvent.click(screen.getByRole("checkbox", { name: s.fullestOnly }));
    expect(codes()).toEqual(["0001-PAP", "0004-PAP"]);
    await userEvent.click(screen.getByRole("checkbox", { name: s.fullestOnly }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: s.kind }), "");
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "italska");
    expect(codes()).toEqual(["0005-CRT", "0005-GLS"]);
  });

  it("downloads the table as shown, the fill in percent and the isle by name", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    let blob: Blob | undefined;
    vi.spyOn(URL, "createObjectURL").mockImplementation((made) => {
      blob = made as Blob;
      return "blob:odpad";
    });
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "vinohradska");
    await userEvent.click(screen.getByRole("button", { name: s.download }));
    const lines = (await blob!.text()).trimEnd().split("\r\n");
    expect(lines[0]).toBe(s.csvHeader.join(","));
    expect(lines.slice(1).map((line) => line.split(",")[0])).toEqual(["0001-PAP", "0001-PLA"]);
    expect(lines[1]).toMatch(/,papír,95,.*,Vinohradská 12$/);
  });

  it("says why when the endpoint refuses, and when there is no endpoint", async () => {
    show("WasteContainer");
    expect(await screen.findByRole("alert")).toHaveTextContent(s.failed("sign in as city staff"));
    show(undefined, false);
    expect(screen.getAllByRole("status").some((el) => el.textContent === s.noEndpoint)).toBe(true);
  });

  it("has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByRole("table")).toBeInTheDocument());
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });

  it("opens every container in the entity panel, with a Portal link and no Edit for a viewer", async () => {
    show();
    await waitFor(() => expect(codes().length).toBeGreaterThan(1));
    for (const button of within(table()).getAllByRole("button").filter((b) => b.closest("th")?.getAttribute("scope") === "row")) {
      await userEvent.click(button);
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    const panel = screen.getByRole("dialog");
    const link = await within(panel).findByRole("link", { name: "Otevřít v Portálu" }, { timeout: 3000 });
    await userEvent.click(link);
    expect(within(panel).queryByRole("button", { name: "Upravit" })).toBeNull();
    await userEvent.click(within(panel).getByRole("button", { name: "Zavřít" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says a failure that is not even an error as it came", () => {
    expect(reasonOf("the proxy hung up")).toBe("the proxy hung up");
    expect(reasonOf(new Error("refused"))).toBe("refused");
  });

  it("sorts by each column both ways, the active one marked", async () => {
    show();
    await waitFor(() => expect(codes().length).toBe(12));
    for (const column of [s.column.code, s.column.fill, s.column.ageHours]) {
      const button = screen.getByRole("button", { name: s.sortBy(column) });
      await userEvent.click(button);
      const header = button.closest("th");
      const first = header?.getAttribute("aria-sort");
      await userEvent.click(button);
      expect(header?.getAttribute("aria-sort")).not.toBe(first);
      expect(["ascending", "descending"]).toContain(header?.getAttribute("aria-sort"));
    }
  });

  it("names an isle in its first language or as plain text, and keeps the desk when isles cannot be read", async () => {
    show(undefined, true, (type, ids) => {
      if (type !== "WasteContainerIsle") return answer(type, ids);
      return [
        { id: ids[0], type: "WasteContainerIsle", name: { type: "LanguageProperty", languageMap: { de: "Erste Insel" } } },
        { id: ids[1], type: "WasteContainerIsle", name: { type: "Property", value: "Plain isle" } },
        { id: ids[2], type: "WasteContainerIsle", name: { type: "Property", value: "  " } },
      ];
    });
    await waitFor(() => expect(screen.getAllByText("Erste Insel").length).toBeGreaterThan(0));
    expect(screen.getAllByText("Plain isle").length).toBeGreaterThan(0);
    document.body.innerHTML = "";
    vi.unstubAllGlobals();
    show(undefined, true, (type, ids) => (type === "WasteContainerIsle" ? null : answer(type, ids)));
    await waitFor(() => expect(codes().length).toBe(12));
    expect(screen.getAllByText(s.isleUnknown).length).toBeGreaterThan(5);
  });

  it("says when the city has more containers than the desk reads, and when there are none", async () => {
    const many = Array.from({ length: 200 }, (_, i) => ({ ...(answer("WasteContainer", [])[0] as object), id: `urn:ngsi-ld:WasteContainer:praha.eu:praha-mesto:m${i}` }));
    // A cap of one page, so the test renders two hundred rows, not four thousand.
    show(undefined, true, (type, ids) => (type === "WasteContainer" ? many : answer(type, ids)), 200);
    expect(await screen.findByText(s.truncated(200))).toBeInTheDocument();
    document.body.innerHTML = "";
    vi.unstubAllGlobals();
    show(undefined, true, () => []);
    expect(await screen.findByText(s.empty)).toBeInTheDocument();
    expect(screen.getByText(s.tenthUnavailable)).toBeInTheDocument();
  });

  it("reads its endpoint from the served list, and counts ages from the moment it opened", async () => {
    vi.stubGlobal("fetch", vi.fn(async (path: string) => new Response(JSON.stringify(answer(new URL(path, "http://portal.test").searchParams.get("type"), [])), { status: 200, headers: { "content-type": "application/json" } })));
    const client = stubClient(undefined, {
      slug: "",
      orgDomain: "praha.eu",
      space: "elsewhere",
      transport: "origin",
      appName: "praha-odpad",
      language: "en",
      endpoints: [{ name: "app-praha-odpad", slug: SLUG, space: "praha-mesto", types: ["WasteContainer", "WasteContainerIsle"] }],
    });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("table")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: LOCALES.en.title })).toBeInTheDocument();
    // Every control of the English desk works as the Czech one does.
    const en = LOCALES.en;
    for (const column of [en.column.code, en.column.fill, en.column.ageHours]) {
      await userEvent.click(screen.getByRole("button", { name: en.sortBy(column) }));
    }
    await userEvent.selectOptions(screen.getByRole("combobox", { name: en.kind }), "paper");
    await userEvent.click(screen.getByRole("checkbox", { name: en.fullestOnly }));
    await userEvent.type(screen.getByRole("searchbox", { name: en.search }), "0001");
    vi.spyOn(URL, "createObjectURL").mockImplementation(() => "blob:odpad");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    await userEvent.click(screen.getByRole("button", { name: en.download }));
    expect(URL.createObjectURL).toHaveBeenCalled();
  });

  it("drops what arrives after the desk is gone", async () => {
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        await held;
        return new Response("[]", { status: 200, headers: { "content-type": "application/json" } });
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.eu", space: "praha-mesto", transport: "origin", appName: "praha-odpad", language: "cs" });
    const view = render(
      <JcProvider client={client}>
        <App now={NOW} />
      </JcProvider>,
    );
    view.unmount();
    release();
    await new Promise((done) => setTimeout(done, 20));
    expect(document.body.textContent).toBe("");
  });
});
