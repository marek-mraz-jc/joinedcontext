import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { ExportButton } from "./ExportButton";

const downloadMock = vi.fn();

vi.mock("@joinedcontext/sdk", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@joinedcontext/sdk")>();
  return { ...actual, download: (...args: unknown[]) => downloadMock(...args) };
});

const rows: Row[] = [
  { id: "urn:1", type: "Alert", name: "Kamppi", category: "traffic" },
  { id: "urn:2", type: "Alert", name: "Pasila" },
];

function show(shown: Row[]) {
  render(
    <JcProvider client={stubClient({}, { endpointName: "helsinki-alerts" })}>
      <ExportButton rows={shown} columns={["name", "category"]} filename="alerts" />
    </JcProvider>,
  );
}

describe("ExportButton", () => {
  beforeEach(() => {
    downloadMock.mockClear();
  });

  it("downloads what is shown as CSV, with the id first, and as a PDF list", async () => {
    show(rows);
    fireEvent.click(screen.getByRole("button", { name: "CSV" }));
    const [csv, csvName] = downloadMock.mock.calls[0] as [Blob, string];
    expect(csvName).toBe("alerts.csv");
    expect((await csv.text()).split(/\r?\n/)[0]).toBe("id,name,category");
    fireEvent.click(screen.getByRole("button", { name: "PDF" }));
    const [pdf, pdfName] = downloadMock.mock.calls[1] as [Blob, string];
    expect(pdfName).toBe("alerts.pdf");
    expect(pdf.type).toBe("application/pdf");
  });

  it("offers nothing to export when nothing is shown", () => {
    show([]);
    expect(screen.getByRole("button", { name: "CSV" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "PDF" })).toBeDisabled();
  });
});
