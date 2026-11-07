// covers: src/components/entities/GridToolbar.tsx src/components/entities/ViewOptions.tsx
/**
 * F-UI1-004..006: two spaces holding the same type share one QueryClient; each count is the one
 * its own space answers, never the other's cached one.
 */
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { EntitySource } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import { FooterAggregate, GroupCount } from "../src/components/entities/GridToolbar";
import { GroupCounts } from "../src/components/entities/ViewOptions";

const sourceOf = (total: number): EntitySource => ({ query: async () => ({ rows: [], total }), get: async () => null });
const STATUS = [{ value: "working", title: "Working" }];

function Space({ scope, total }: { scope: string; total: number }) {
  const source = sourceOf(total);
  return (
    <section aria-label={scope}>
      <FooterAggregate scope={scope} source={source} type="Event" attr="seats" label="Seats" value="count" onChange={() => undefined} />
      <GroupCount scope={scope} source={source} type="Event" path={[{ attr: "status", value: "open" }]} />
      <GroupCounts scope={scope} source={source} type="Event" attr="status" options={STATUS} chosen={null} onChoose={() => undefined} />
    </section>
  );
}

describe("the counts of two spaces of one type", () => {
  it("each show their own space's answer", async () => {
    await i18n.changeLanguage("en");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={client}>
          <Space scope="helsinki/parking" total={7} />
          <Space scope="helsinki/events" total={40} />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    const first = screen.getByRole("region", { name: "helsinki/parking" });
    const second = screen.getByRole("region", { name: "helsinki/events" });
    expect(await within(first).findByText("7 rows")).toBeInTheDocument();
    expect(await within(second).findByText("40 rows")).toBeInTheDocument();
    expect(await within(first).findByRole("button", { name: "Working (7)" })).toBeInTheDocument();
    expect(await within(second).findByRole("button", { name: "Working (40)" })).toBeInTheDocument();
    expect(within(first).getByText("7")).toBeInTheDocument();
    expect(within(second).getByText("40")).toBeInTheDocument();
  });
});
