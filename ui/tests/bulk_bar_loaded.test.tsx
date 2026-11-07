// covers: src/pages/spaces/SpaceInside.tsx
/** F-UI3-014: the bulk actions act on loaded rows; the bar says so when not every checked row is loaded. */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { RichRow } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import { BulkBar } from "../src/pages/spaces/SpaceInside";

const row = (n: number) => ({ id: `urn:ngsi-ld:Event:${n}`, cells: {} }) as unknown as RichRow;
const source = { query: async () => ({ rows: [] }), get: async () => null } as never;

function show(count: number, loaded: number) {
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient()}>
        <BulkBar
          project="helsinki"
          space="events"
          source={source}
          count={count}
          rows={Array.from({ length: loaded }, (_, n) => row(n))}
          canDelete
          writable={[]}
          writeSource={source}
          onChanged={() => undefined}
          exportLinks={null}
          tooManyToExport={false}
          onClear={() => undefined}
          onDeleted={() => undefined}
        />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

describe("the bulk bar", () => {
  it("says how many of the checked rows Delete and Set act on", async () => {
    await i18n.changeLanguage("en");
    show(500, 100);
    expect(screen.getByText("100 of 500 are loaded: Delete selected and Set a field act on those 100. Scroll the grid to load the rest first.")).toBeInTheDocument();
  });

  it("says nothing more when every checked row is loaded", async () => {
    await i18n.changeLanguage("en");
    show(3, 3);
    expect(screen.queryByText(/are loaded/)).not.toBeInTheDocument();
  });
});
