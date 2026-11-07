import { download, format, toCsv, toPdf, useClient } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { t } from "../i18n";

/** What the desk exports: the rows shown, as CSV or as a PDF list of at most 500 lines. */
export function ExportButton({ rows, columns, filename, title = filename }: { rows: Row[]; columns: string[]; filename: string; title?: string }): React.JSX.Element {
  const client = useClient();
  const empty = rows.length === 0;

  const handleCsv = () => {
    download(toCsv(["id", ...columns], rows), `${filename}.csv`);
  };

  const handlePdf = () => {
    const lines = rows.slice(0, 500).map((r) => [r.id, ...columns.map((c) => format(r[c]))].join(" · "));
    const blob = toPdf({ title, endpoint: client.config.endpointName ?? "", filters: "", takenAt: new Date().toISOString(), lines });
    download(blob, `${filename}.pdf`);
  };

  return (
    <div className="jc-export" role="group" aria-label={t("export.label")}>
      <button type="button" disabled={empty} onClick={handleCsv}>
        CSV
      </button>
      <button type="button" disabled={empty} onClick={handlePdf}>
        PDF
      </button>
    </div>
  );
}
