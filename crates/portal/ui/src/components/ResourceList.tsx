import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Table, TableBody, TableEmpty, TableSkeleton } from "./ui";
import { PageFailed } from "./ui/PageState";

/**
 * The table of a resource list page (T-1382): a skeleton while the list loads, the API's own
 * reason with a retry when it fails, the empty state when it holds nothing, the rows otherwise.
 * The page keeps its header, its actions and what it draws around the table.
 */
export function ResourceList({
  query,
  caption,
  head,
  columns,
  count,
  empty,
  children,
}: {
  query: { isPending: boolean; isError: boolean; error: unknown; refetch: () => unknown };
  caption: string;
  head: ReactNode;
  columns: number;
  /** How many rows the list holds; none shows `empty`. */
  count: number;
  /** An `EmptyState`, `bare`, with the page's create action when it has one (T-1381). */
  empty: ReactNode;
  children: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  if (query.isError) {
    // The one failure panel (T-3244): the API's sentence, its reference to copy, and a retry only
    // where asking again can help.
    return (
      <PageFailed
        error={query.error}
        onRetry={() => {
          void query.refetch();
        }}
      />
    );
  }
  return (
    <Table data-records="" caption={caption} status={query.isPending ? t("app.loading") : undefined}>
      {head}
      {query.isPending ? (
        <TableSkeleton columns={columns} />
      ) : (
        <TableBody>
          {count === 0 ? <TableEmpty columns={columns}>{empty}</TableEmpty> : children}
        </TableBody>
      )}
    </Table>
  );
}
