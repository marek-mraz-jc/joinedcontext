/**
 * The shared grid for a public page (T-6752): the grid loads with the first table it draws, so a
 * public route's own chunk stays inside its budget (scripts/bundle-budget.mjs).
 */
import { lazy, Suspense } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { loadGridLocales } from "../../i18n";
import type { AdminGridProps } from "./AdminGrid";

// `lazy` takes no type parameter of its own: the generic signature is AdminGrid's, restated.
// Its words come with it (i18n.ts loadGridLocales): outside the public groups, they would load the whole locale.
const Grid = lazy(() =>
  Promise.all([import("./AdminGrid"), loadGridLocales()]).then(([module]) => ({ default: module.AdminGrid })),
) as unknown as <T>(
  props: AdminGridProps<T>,
) => JSX.Element;

export function LazyAdminGrid<T>(props: AdminGridProps<T>): JSX.Element {
  const { t } = useTranslation();
  return (
    <Suspense fallback={<p role="status" className="text-body text-fg-muted">{t("app.loading")}</p>}>
      <Grid {...props} />
    </Suspense>
  );
}
