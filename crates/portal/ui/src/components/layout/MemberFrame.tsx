import { Suspense } from "react";
import type { JSX, ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../auth/AuthProvider";
import { Button, Icon, PageLoading } from "../ui";
import type { IconName } from "../ui/icons";
import { BrandMark } from "./BrandMark";

const LINKS: { to: "/catalogue" | "/ai-connectors" | "/settings"; label: string; icon: IconName }[] = [
  { to: "/catalogue", label: "member.catalogue", icon: "ckan" },
  { to: "/ai-connectors", label: "member.connectors", icon: "access" },
  { to: "/settings", label: "member.settings", icon: "user" },
];

/**
 * The shell of a member (ADR-N-067 §2.4, T-6411): a signed-in person with no project, so no
 * project, Context Space or Pipeline page. They read the catalogue, their settings and their AI
 * connectors; everything else the server answers 404 for them anyway.
 */
export function MemberFrame({ children }: { children: ReactNode }): JSX.Element {
  const { t } = useTranslation();
  const { identity, signOut } = useAuth();
  const who = identity?.name ?? identity?.username;
  return (
    <div className="flex min-h-screen flex-col bg-bg font-sans text-fg">
      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border bg-surface px-4 py-2">
        <BrandMark short />
        <nav aria-label={t("member.nav")} className="order-last flex w-full gap-1 overflow-x-auto sm:order-none sm:w-auto">
          {LINKS.map((link) => (
            <Link
              key={link.to}
              to={link.to}
              className="focus-ring inline-flex shrink-0 items-center gap-1.5 rounded-md px-3 py-1.5 text-body text-fg-muted hover:bg-surface-muted hover:text-fg"
              activeProps={{ "aria-current": "page", className: "bg-primary-soft text-primary-soft-fg" }}
            >
              <Icon name={link.icon} className="size-4" />
              {t(link.label)}
            </Link>
          ))}
        </nav>
        <div className="flex items-center gap-2">
          {who ? <span className="hidden text-caption text-fg-muted md:inline">{who}</span> : null}
          <Button size="sm" variant="secondary" icon={<Icon name="logout" className="size-4" />} onClick={() => void signOut()}>
            {t("member.signOut")}
          </Button>
        </div>
      </header>
      <main id="main" className="mx-auto min-h-dvh w-full max-w-7xl flex-1 p-6">
        <Suspense fallback={<PageLoading label={t("app.loading")} />}>{children}</Suspense>
      </main>
    </div>
  );
}
