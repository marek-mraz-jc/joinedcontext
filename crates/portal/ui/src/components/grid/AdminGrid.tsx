/**
 * An admin list on the shared data grid (T-6739, owner 2026-11-11: one grid across the open-data
 * explorer, the Portal and Apps): the SDK's `DataGrid` with the Portal's words and its one failure
 * panel. The rows are what the list's own read returned, the server's permission check included;
 * the grid only searches, filters, sorts and pages them. A click on a row opens its `data-row-link`.
 */
import { useMemo } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { DataGrid } from "@joinedcontext/sdk/grid";
import type { DataColumn, DataGridLabels } from "@joinedcontext/sdk/grid";
import { gridLabels } from "../entities/PortalEntityGrid";
import { PageFailed } from "../ui/PageState";
import { recordCard } from "../ui";

export type { DataColumn };

export function AdminGrid<T>({
  query,
  caption,
  rows,
  columns,
  rowKey,
  empty,
  storageKey,
  stack,
  noSearch,
}: {
  /** The list's read: the skeleton while it runs, the API's reason and a retry when it fails. */
  query: { isPending: boolean; isError: boolean; error: unknown; refetch: () => unknown };
  caption: string;
  rows: readonly T[];
  columns: readonly DataColumn<T>[];
  rowKey: (row: T) => string;
  /** An `EmptyState`, `bare`, with the page's create action when it has one. */
  empty: ReactNode;
  /** Keys the columns a person arranged in this browser. */
  storageKey: string;
  stack?: boolean;
  /** The page searches the list itself, with its own words. */
  noSearch?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const labels = useMemo<DataGridLabels>(
    () => ({
      ...gridLabels(t),
      searchRows: t("dataGrid.searchRows"),
      searchRowsHint: t("dataGrid.searchRowsHint"),
      noMatch: t("dataGrid.noMatch"),
      rowCount: t("dataGrid.rowCount", { n: "{n}" }),
    }),
    [t],
  );
  return (
    <DataGrid
      caption={caption}
      rows={rows}
      columns={columns}
      rowKey={rowKey}
      loading={query.isPending}
      error={
        query.isError ? (
          <PageFailed
            error={query.error}
            onRetry={() => {
              void query.refetch();
            }}
          />
        ) : undefined
      }
      empty={empty}
      labels={labels}
      storageKey={`portal:${storageKey}`}
      stack={stack}
      noSearch={noSearch}
      // A click on a plain cell opens the row's record link, as on every list (T-2875).
      rowProps={() => recordCard}
    />
  );
}
