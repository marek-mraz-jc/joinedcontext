import { Suspense } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../auth/AuthProvider";
import { Button, PageLoading } from "../ui";
import { BrandMark } from "./BrandMark";

/**
 * The frame of a page anybody may open (EP-81): the brand, a sign-in button for a visitor, the
 * page, and the footer every public page must carry. Both the public entry (publicRouter.tsx,
 * T-5122) and the full router draw it.
 */
export function PublicFrame({ children, anonymous = false }: { children: React.ReactNode; anonymous?: boolean }): React.JSX.Element {
  const { t } = useTranslation();
  const { status, signIn } = useAuth();
  return (
    <div className="flex min-h-screen flex-col bg-bg font-sans text-fg">
      <header className="flex h-14 items-center justify-between gap-4 border-b border-border bg-surface px-4">
        <BrandMark short />
        {status === "anonymous" && !anonymous ? (
          <Button size="sm" variant="secondary" onClick={() => signIn(window.location.pathname + window.location.search)}>
            {t("auth.signIn")}
          </Button>
        ) : null}
      </header>
      {/* At least a screen tall: the footer starts below the fold, so the page drawing in above it
          moves nothing a reader sees (T-5165, layout shift). */}
      <main id="main" className="mx-auto min-h-dvh w-full max-w-7xl flex-1 p-6">
        <Suspense fallback={<PageLoading label={t("app.loading")} />}>{children}</Suspense>
      </main>
      {/* The statement a public body must link from every page (T-5108). */}
      <footer className="border-t border-border bg-surface px-4 py-3 text-caption">
        <nav aria-label={t("accessibility.footer")} className="mx-auto flex max-w-7xl flex-wrap gap-x-4 gap-y-1">
          <a className="text-primary-soft-fg underline hover:no-underline" href="/accessibility">
            {t("accessibility.link")}
          </a>
          <a className="text-primary-soft-fg underline hover:no-underline" href="/glossary">
            {t("glossary.page.title")}
          </a>
        </nav>
      </footer>
    </div>
  );
}
