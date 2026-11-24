// T-7706: the data source wizard edges: a bad address for the paging guess, a CKAN dataset page, clearing a refusal, the pages and rows boxes.
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { TemplateGallery } from "../src/pages/datasources/TemplateGallery";
import { json, renderPage } from "./page_contract";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function renderWizard(answer: (path: string) => Response | undefined = () => undefined, secrets: string[] = []) {
  const onUse = vi.fn();
  const asked: string[] = [];
  renderPage(<TemplateGallery project="helsinki" secrets={secrets} others={[]} onUse={onUse} onPick={vi.fn()} />, {
    path: "/projects/helsinki/datasources",
    answer: async (url, request) => {
      if (request.method !== "POST") return undefined;
      asked.push(url.pathname);
      return answer(url.pathname);
    },
  });
  await screen.findByTestId("datasource-templates");
  return { onUse, asked };
}

const jsonCard = () => screen.getByRole("button", { name: (name) => name.startsWith(en.datasources.template.json.title) });

describe("the data source wizard, the edges", () => {
  it("asks nothing for the paging guess of an address that is no address, and clears the refusal once it is typed again", async () => {
    const user = userEvent.setup();
    const { asked } = await renderWizard();
    await user.click(jsonCard());
    const address = screen.getByLabelText(new RegExp(en.datasources.template.json.url));
    await user.type(address, "not an address");
    await user.click(screen.getByRole("button", { name: en.datasources.wizard.pagingButton }));
    expect(await screen.findByText(en.datasources.template.problem.url)).toBeInTheDocument();
    expect(asked).toEqual([]);
    await user.type(address, "x");
    expect(screen.queryByText(en.datasources.template.problem.url)).not.toBeInTheDocument();
  });

  it("forgets the missing-key refusal when another secret is picked", async () => {
    const user = userEvent.setup();
    const { onUse } = await renderWizard(() => undefined, ["key-a", "key-b"]);
    await user.click(jsonCard());
    await user.type(screen.getByLabelText(new RegExp(en.datasources.template.json.url)), "https://services.example/Customers");
    await user.selectOptions(screen.getByLabelText(en.datasources.wizard.secret), "key-a");
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(screen.getByText(en.datasources.template.problem.secretKey)).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText(en.datasources.wizard.secret), "key-b");
    expect(screen.queryByText(en.datasources.template.problem.secretKey)).not.toBeInTheDocument();
    expect(onUse).not.toHaveBeenCalled();
  });

  it("leaves the walk out of the form when the pages box is unticked, and the CSV rows when theirs is", async () => {
    const user = userEvent.setup();
    const { onUse } = await renderWizard((path) =>
      path.endsWith("/datasource-paging") ? json({ paging: { mode: "nextUrl", next: "next" }, records: "value", onPage: 20, more: true }) : undefined,
    );
    await user.click(jsonCard());
    await user.type(screen.getByLabelText(new RegExp(en.datasources.template.json.url)), "https://services.example/Customers");
    await user.click(screen.getByRole("button", { name: en.datasources.wizard.pagingButton }));
    const pages = await screen.findByRole("checkbox", { name: new RegExp(en.datasources.wizard.allPages) });
    expect(pages).toBeChecked();
    await user.click(pages);
    expect(pages).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    expect(onUse).toHaveBeenLastCalledWith("http", { name: "customers", http: { url: "https://services.example/Customers", verb: "GET", records: "value" } });

    await user.click(screen.getByRole("button", { name: new RegExp(en.datasources.template.csv.title) }));
    const rows = screen.getByRole("checkbox", { name: new RegExp(en.datasources.wizard.rows) });
    await user.click(rows);
    expect(rows).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: en.datasources.template.use }));
    await waitFor(() => expect(onUse).toHaveBeenCalledTimes(2));
    expect(onUse).toHaveBeenLastCalledWith("http", { name: "customers", http: { url: "https://services.example/Customers", verb: "GET" } });
  });
});
