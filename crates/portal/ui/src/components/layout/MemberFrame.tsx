import { Suspense } from "react";
import type { JSX, ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../auth/AuthProvider";
import { useBranding } from "../../branding";
import { useAdministers, useMayOpenProject } from "../../api/permissions";
import { LanguageSwitcher } from "../LanguageSwitcher";
import { Button, Icon, PageLoading } from "../ui";
import type { IconName } from "../ui/icons";
import { BrandMark } from "./BrandMark";
import { ShellFooter } from "./ShellFooter";

type MemberLink = { to: "/projects" | "/catalogue" | "/ai-connectors" | "/settings"; label: string; icon: IconName };

/** Projects, for whoever may open one: there they open their first (T-7380). */
const PROJECTS: MemberLink = { to: "/projects", label: "member.projects", icon: "folder" };

const LINKS: MemberLink[] = [
  { to: "/catalogue", label: "member.catalogue", icon: "ckan" },
  { to: "/ai-connectors", label: "member.connectors", icon: "access" },
  { to: "/settings", label: "member.settings", icon: "user" },
];

const MEMBER_LINK =
  "focus-ring inline-flex shrink-0 items-center gap-1.5 rounded-md px-3 py-1.5 text-body text-fg-muted hover:bg-surface-muted hover:text-fg";

/**
 * The shell of a member (ADR-N-067 §2.4, T-6411): a signed-in person with no project, so no
 * project, Context Space or Pipeline page. They read the catalogue, their settings and their AI
 * connectors; everything else the server answers 404 for them anyway.
 */
export function MemberFrame({ children }: { children: ReactNode }): JSX.Element {
  const { t } = useTranslation();
  const { identity, signOut } = useAuth();
  const branding = useBranding();
  const who = identity?.name ?? identity?.username;
  const may = useMayOpenProject();
  // An Organization's own administrator with no project reaches its Administration from here too
  // (T-7380); its pages and the API decide what opens.
  const { opensAdministration } = useAdministers();
  const links = may.allowed ? [PROJECTS, ...LINKS] : LINKS;
  // The Shell's skip link, language switch and footer, the accessibility statement in it: every
  // page carries them (Decree 78/2020, T-5108), a member's too.
  return (
    <div className="flex min-h-screen flex-col bg-bg font-sans text-fg">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50 focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-primary-fg"
      >
        {t("nav.skipToContent")}
      </a>
      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-border bg-surface px-4 py-2">
        <BrandMark short />
        <nav aria-label={t("member.nav")} className="order-last flex w-full gap-1 overflow-x-auto sm:order-none sm:w-auto">
          {links.map((link) => (
            <Link
              key={link.to}
              to={link.to}
              className={MEMBER_LINK}
              activeProps={{ "aria-current": "page", className: "bg-primary-soft text-primary-soft-fg" }}
            >
              <Icon name={link.icon} className="size-4" />
              {t(link.label)}
            </Link>
          ))}
          {opensAdministration ? (
            <Link to="/organization/$tab" params={{ tab: "members" }} className={MEMBER_LINK}>
              <Icon name="lock" className="size-4" />
              {t("member.administration")}
            </Link>
          ) : null}
        </nav>
        <div className="flex items-center gap-2">
          {who ? <span className="hidden text-caption text-fg-muted md:inline">{who}</span> : null}
          <LanguageSwitcher />
          <Button size="sm" variant="secondary" icon={<Icon name="logout" className="size-4" />} onClick={() => void signOut()}>
            {t("member.signOut")}
          </Button>
        </div>
      </header>
      <main id="main" className="mx-auto min-h-dvh w-full max-w-7xl flex-1 p-6">
        <Suspense fallback={<PageLoading label={t("app.loading")} />}>{children}</Suspense>
      </main>
      <ShellFooter organisation={branding.organisation} contactEmail={branding.contactEmail} />
    </div>
  );
}
