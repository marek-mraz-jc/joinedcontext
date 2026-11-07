import { Fragment, Suspense, useEffect, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { Link, useMatchRoute, useRouterState } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { LanguageSwitcher } from "../LanguageSwitcher";
import { NotificationsMenu } from "../../pages/spaces/Comments";
import { useDecisions } from "../../api/decision";
import { HelpMenu } from "../HelpMenu";
import { FeedbackButton } from "../FeedbackButton";
import { JobsMenu } from "../JobsMenu";
import { AssistantDock } from "../../assistant/AssistantDock";
import { PairedTabBar } from "../../connectors/PairedTabBar";
import { CommandPalette } from "../../navigation/CommandPalette";
import { PageTools } from "../../navigation/PageTools";
import { useAuth } from "../../auth/AuthProvider";
import { localized } from "../../api/manifest";
import { lastAddress, rememberAddress, rememberProject, useOrganizations, useProjects, useProjectTitles } from "../../api/projects";
import { useAdministers } from "../../api/permissions";
import { useBranding, useHiddenSections } from "../../branding";
import {
  Alert,
  Button,
  buttonClass,
  Icon,
  Menu,
  MenuContent,
  MenuItem,
  MenuLabel,
  MenuSeparator,
  MenuTrigger,
  PageLoading,
} from "../ui";
import type { IconName } from "../ui";
import { isHiddenSection, NAV_SECTIONS, sameSection } from "./navigation";
import { ErrorBoundary } from "../ErrorBoundary";
import { NewProjectButton } from "./NewProject";
import { WorkspaceBar } from "./WorkspaceBar";
import { withoutCopy } from "./WorkspaceContext";
import { BrandMark, navLinkClass } from "./BrandMark";
import { ShellFooter } from "./ShellFooter";

export { BrandMark, navLinkClass };

/** Switches between the projects the repository holds; the active one is listed whatever the API says. */
function ProjectSelector({ active }: { active: string }) {
  const { t } = useTranslation();
  const known = useProjects().data ?? [];
  const titleOf = useProjectTitles();
  // With several Organizations the list is the chosen one's (PF-124); the active project stays.
  const organization = useChosenOrganization(active);
  const { ownerOf } = useOrganizations().data ?? { ownerOf: new Map<string, string | undefined>() };
  const shown = organization ? known.filter((name) => ownerOf.get(name) === organization) : known;
  const projects = shown.includes(active) ? shown : [active, ...shown];
  // The page in hand, so the switch keeps it: a person comparing two projects' endpoints stays
  // in Endpoints instead of walking back from Spaces every time (T-2425, UI-05).
  const here = useRouterState({ select: (state) => sameSection(state.location.pathname) });
  // A project left earlier reopens on the page the person left there, its space and type
  // included (T-5603); one never visited in this browser keeps the section.
  // A built `href` stands in for `to`, `params` and `search` when there is one.
  const switchTo = (project: string) => ({
    to: "/projects/$project/$plural" as const,
    params: { project, plural: here.plural },
    search: withoutCopy({}),
    href: project === active ? undefined : lastAddress(project),
  });
  return (
    <Menu>
      <MenuTrigger asChild>
        <Button
          variant="secondary"
          size="lg"
          aria-label={t("nav.projects")}
          className="w-full justify-start gap-2.5 px-2.5 text-left"
        >
          <span className="inline-flex size-7 shrink-0 items-center justify-center rounded-md bg-primary text-caption font-bold uppercase text-primary-fg">
            {active.slice(0, 2)}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-body font-semibold text-fg">{titleOf(active)}</span>
            {/* The slug under the title, the name every path and manifest uses (T-4243). */}
            <span aria-hidden={titleOf(active) === active || undefined} className="block truncate font-mono text-caption text-fg-muted">
              {titleOf(active) !== active ? active : "\u00a0"}
            </span>
          </span>
          <Icon name="chevronDown" className="size-4 text-fg-subtle" />
        </Button>
      </MenuTrigger>
      {/* The trigger's own width, which Radix measures and publishes: a computed size, not one
          the spacing scale could name. */}
      <MenuContent align="start" className="w-[var(--radix-dropdown-menu-trigger-width)]">
        <MenuLabel>{t("nav.projects")}</MenuLabel>
        {projects.map((project) => (
          <MenuItem key={project} asChild>
            <Link
              {...switchTo(project)}
              aria-current={project === active ? "true" : undefined}
            >
              {project === active ? <Icon name="check" className="size-4 shrink-0" /> : <span className="size-4 shrink-0" />}
              {/* Every row the same two lines: the title, then the slug every path uses (owner 2026-10-30). */}
              <span className="min-w-0 flex-1">
                <span className="block truncate">{titleOf(project)}</span>
                {/* Without a title the slug is the first line, and the second keeps the row's height. */}
                <span
                  aria-hidden={titleOf(project) === project || undefined}
                  className="block truncate font-mono text-caption text-fg-muted"
                >
                  {titleOf(project) !== project ? project : "\u00a0"}
                </span>
              </span>
            </Link>
          </MenuItem>
        ))}
      </MenuContent>
    </Menu>
  );
}

/**
 * The Organization the person works in (PF-124): `?org=` when it names one they see, else the
 * active project's. `undefined` when they see one Organization only.
 */
function useChosenOrganization(active: string): string | undefined {
  const { organizations, ownerOf } = useOrganizations().data ?? { organizations: [], ownerOf: new Map() };
  const asked = useRouterState({ select: (state) => (state.location.search as { org?: string }).org });
  if (organizations.length < 2) {
    return undefined;
  }
  if (asked && organizations.some((organization) => organization.name === asked)) {
    return asked;
  }
  return ownerOf.get(active) ?? undefined;
}

/**
 * Switches between the Organizations the person sees, for a person who sees more than one
 * (PF-124, ADR-N-058 section 2.7). The choice is kept in `?org=` and opens that Organization's
 * first project on the page in hand; the server listed only Organizations owning a project the
 * person reads, so every entry opens something.
 */
function OrganizationSwitcher({ active, inSidebar = false }: { active: string; inSidebar?: boolean }) {
  const { t, i18n } = useTranslation();
  const data = useOrganizations().data;
  const known = useProjects().data ?? [];
  const chosen = useChosenOrganization(active);
  const here = useRouterState({ select: (state) => sameSection(state.location.pathname) });
  if (!data || data.organizations.length < 2) {
    return null;
  }
  const titleOf = (name: string) =>
    localized(data.organizations.find((organization) => organization.name === name)?.title, i18n.language, name);
  return (
    <Menu>
      <MenuTrigger asChild>
        <Button
          variant={inSidebar ? "secondary" : "ghost"}
          size="sm"
          aria-label={t("nav.organizationSwitch", { organization: chosen ? titleOf(chosen) : "" })}
          title={t("nav.organizationHint")}
          className={inSidebar ? "w-full justify-start gap-1.5" : "max-w-48 gap-1.5"}
        >
          <Icon name="user" className="size-4 shrink-0" />
          <span className="min-w-0 flex-1 truncate text-left">{chosen ? titleOf(chosen) : t("nav.organizationChoose")}</span>
          <Icon name="chevronDown" className="size-4 shrink-0 text-fg-subtle" />
        </Button>
      </MenuTrigger>
      <MenuContent align={inSidebar ? "start" : "end"} className="min-w-56 max-w-80">
        <MenuLabel>
          {t("nav.organizationLabel")}
          <span className="block text-caption font-normal text-fg-muted">{t("nav.organizationHint")}</span>
        </MenuLabel>
        {data.organizations.map((organization) => {
          const first = known.find((name) => data.ownerOf.get(name) === organization.name);
          if (!first) {
            return null;
          }
          return (
            <MenuItem key={organization.name} asChild>
              <Link
                to="/projects/$project/$plural"
                params={{ project: organization.name === data.ownerOf.get(active) ? active : first, plural: here.plural }}
                search={withoutCopy({ org: organization.name })}
                aria-current={organization.name === chosen ? "true" : undefined}
              >
                {organization.name === chosen ? <Icon name="check" className="size-4" /> : <span className="size-4" />}
                <span className="min-w-0">
                  <span className="block truncate">{titleOf(organization.name)}</span>
                  <span className="block truncate font-mono text-caption text-fg-muted">{organization.name}</span>
                </span>
              </Link>
            </MenuItem>
          );
        })}
      </MenuContent>
    </Menu>
  );
}

/** The sections with a route of their own: the generic `$plural` template would resolve to it anyway, with a router warning. */
const OWN_ROUTES = {
  home: "/projects/$project/home",
  approvals: "/projects/$project/approvals",
  assistant: "/projects/$project/assistant",
} as const;

function ownRoute(plural: string): (typeof OWN_ROUTES)[keyof typeof OWN_ROUTES] | undefined {
  return Object.hasOwn(OWN_ROUTES, plural) ? OWN_ROUTES[plural as keyof typeof OWN_ROUTES] : undefined;
}

function UserMenu() {
  const { t } = useTranslation();
  const { identity, signOut } = useAuth();
  const { opensAdministration } = useAdministers();
  if (!identity) {
    return null;
  }
  const display = identity.name ?? identity.username;
  const initial = display.trim().charAt(0).toUpperCase();
  return (
    <Menu>
      <MenuTrigger asChild>
        <Button
          variant="ghost"
          aria-label={t("auth.signedInAs", { name: display })}
          className="gap-2 pointer-coarse:max-sm:px-1.5"
        >
          <span className="inline-flex size-7 items-center justify-center rounded-full bg-primary-soft text-caption font-bold text-primary-soft-fg">
            {initial}
          </span>
          <span className="hidden max-w-40 truncate text-body font-medium text-fg sm:block">
            {display}
          </span>
          <Icon name="chevronDown" className="hidden size-4 text-fg-subtle sm:block" />
        </Button>
      </MenuTrigger>
      <MenuContent align="end" className="min-w-52">
        <MenuLabel>
          <span className="block truncate text-body font-medium text-fg">{display}</span>
          {identity.name ? <span className="block truncate font-mono">{identity.username}</span> : null}
        </MenuLabel>
        <MenuSeparator />
        {/* The account menu holds the person and, for a platform administrator, Administration
            (owner 2026-11-01); help pages and AI connectors live elsewhere. */}
        <MenuItem asChild>
          <Link to="/settings">
            <Icon name="user" className="size-4" />
            {t("settingsPage.title")}
          </Link>
        </MenuItem>
        {opensAdministration ? (
          <MenuItem asChild>
            <Link to="/organization/$tab" params={{ tab: "settings" }}>
              <Icon name="access" className="size-4" />
              {t("nav.organization")}
            </Link>
          </MenuItem>
        ) : null}
        <MenuSeparator />
        <MenuItem
          onSelect={() => {
            void signOut();
          }}
        >
          <Icon name="logout" className="size-4" />
          {t("auth.signOut")}
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}

function NavLabel({ icon, label }: { icon: IconName; label: string }) {
  return (
    <>
      <Icon name={icon} className="size-4.5" />
      <span className="truncate">{label}</span>
    </>
  );
}

/**
 * The changes waiting for this person's approval (T-1391): pending, not their own unless they
 * administer the kind, on a kind they may approve. The same list and key as the Approvals page,
 * polled every 30 s; a list the API refuses counts as none.
 */
/** The header's inbox: its mentions and the changes this person may decide here (T-3273). */
function InboxButton({ project }: { project: string }) {
  return <NotificationsMenu project={project} waiting={useDecisions(project).length} />;
}

function PendingBadge({ project }: { project: string }) {
  const { t } = useTranslation();
  const count = useDecisions(project).length;
  if (count === 0) return null;
  return (
    <span
      className="ml-auto rounded-full bg-danger px-1.5 text-caption font-semibold text-danger-fg"
      title={t("nav.approvalsPending", { count })}
    >
      <span aria-hidden="true">{count}</span>
      <span className="sr-only">{t("nav.approvalsPending", { count })}</span>
    </span>
  );
}

/**
 * What one page that threw leaves behind, inside the chrome rather than instead of it (T-2426).
 *
 * Not `PageFailed` from `components/ui`, which is the answer to a request that failed and offers
 * the retry that repeats it: this one is a render error, so it also carries the reference tying
 * it to the console line and the way to a page that still works.
 *
 * The router renders the shell and the page as one component, so before this an error anywhere
 * in a page replaced the header, the sidebar and the project switcher with a panel — and the
 * person had the browser's Back button and nothing else. The chrome stays; this sits where the
 * page was, says what happened, and offers the two things that help: draw this page again, or
 * go to the project's start. What threw is on the console and nowhere near the DOM (PF-50).
 */
function PageStopped({
  project,
  reference,
  retry,
}: {
  project: string;
  reference: string;
  retry: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Alert
      tone="danger"
      title={t("app.error.crashTitle")}
      actions={
        <>
          <Button variant="primary" size="sm" onClick={retry}>
            {t("app.error.retry")}
          </Button>
          <Link
            to="/projects/$project/$plural"
            params={{ project, plural: NAV_SECTIONS[0].plural }}
            className={buttonClass("secondary", "sm")}
          >
            {t("app.error.crashHome")}
          </Link>
        </>
      }
    >
      <p>{t("app.error.crashBody")}</p>
      <p className="mt-2 text-caption">
        {t("app.error.crashReference")}{" "}
        <code data-testid="error-reference" className="font-mono">
          {reference}
        </code>
      </p>
    </Alert>
  );
}

/**
 * `useMatchRoute`, answering "no match" where the router cannot decode the address: a fuzzy match
 * of a broken escape throws, and the Shell sits above every page's error boundary (F-UI1-002).
 */
function useSafeMatchRoute(): ReturnType<typeof useMatchRoute> {
  const matchRoute = useMatchRoute();
  return ((options: Parameters<typeof matchRoute>[0]) => {
    try {
      return matchRoute(options);
    } catch {
      return false;
    }
  }) as ReturnType<typeof useMatchRoute>;
}

/** The static pages under `/projects/{project}/spaces/`, by their crumb's words. */
const SPACE_PAGES: Partial<Record<string, string>> = {
  complete: "spaces.complete.title",
  upload: "spaces.upload.title",
};

/** One segment of the address as a person reads it; a broken escape stays as typed (F-UI1-002). */
function decodedSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

/**
 * The one thing the page is about, from the address (T-3239): any item of a section at
 * `/projects/{project}/{plural}/{name}…`, whose own address opens it. A page of its own under
 * Context Spaces is named in words (`spacePage`, an i18n key): `/spaces/complete` is no space
 * called "complete" in a monospace crumb.
 */
export function addressItem(pathname: string): { itemName?: string; spacePage?: string; creating?: boolean } {
  const segments = pathname.split("/").filter(Boolean).map(decodedSegment);
  if (segments[0] !== "projects" || segments.length < 4) return {};
  const spacePage = segments[2] === "spaces" && segments.length === 4 ? SPACE_PAGES[segments[3] ?? ""] : undefined;
  if (spacePage) return { spacePage };
  return segments[3] === "new" ? { creating: true } : { itemName: segments[3] };
}

/**
 * The crumb of a section page: an approval or a copy, a Context Spaces page of its own, an item
 * named by the address, or a create form lead back to the section's list; the list itself is
 * named without a link.
 */
export function SectionCrumb({
  project,
  plural,
  label,
  detail,
  spacePage,
  itemName,
  creating,
}: {
  project: string;
  plural: string;
  label: string;
  detail: { to: "/projects/$project/approvals" | "/projects/$project/workspaces"; name: string } | null;
  spacePage?: string;
  itemName?: string;
  creating?: boolean;
}) {
  const { t } = useTranslation();
  const list = (
    <Link
      to="/projects/$project/$plural"
      params={{ project, plural }}
      className="focus-ring touch-target rounded-sm hover:text-fg hover:underline"
    >
      {label}
    </Link>
  );
  const chevron = <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />;
  if (detail) {
    return (
      <>
        <Link
          to={detail.to}
          params={{ project }}
          className="focus-ring touch-target rounded-sm hover:text-fg hover:underline"
        >
          {label}
        </Link>
        {chevron}
        <span aria-current="page" className="font-mono font-medium text-fg">
          {detail.name}
        </span>
      </>
    );
  }
  if (spacePage) {
    return (
      <>
        {list}
        {chevron}
        <span aria-current="page" className="font-medium text-fg">
          {t(spacePage)}
        </span>
      </>
    );
  }
  if (itemName && plural !== "settings") {
    return (
      <>
        {list}
        {chevron}
        <Link
          to="/projects/$project/$plural/$name"
          params={{ project, plural, name: itemName }}
          aria-current="page"
          className="focus-ring rounded-sm font-mono font-medium text-fg hover:underline"
        >
          {itemName}
        </Link>
      </>
    );
  }
  // A create form: the section leads back to its list (owner 2026-10-31, no "Back to the list").
  if (creating) return list;
  return (
    <span aria-current="page" className="font-medium text-fg">
      {label}
    </span>
  );
}

/**
 * The model the Data models crumb names: the model of the address or of the editor's `?edit=`,
 * "" for a new model, undefined off the models pages or on the list itself.
 */
export function crumbModel(modelsActive: boolean, itemName: string | undefined, editingModel: string | undefined): string | undefined {
  if (!modelsActive) return undefined;
  return itemName ?? editingModel;
}

/**
 * The crumb of a page outside the sections; one model's page (or its editor) links back to Data
 * models and names the model (owner 2026-10-31). `model` is "" for a new model, undefined elsewhere.
 */
export function OtherPageCrumb({ project, label, model }: { project: string; label: string; model: string | undefined }) {
  if (model === undefined) {
    return (
      <span aria-current="page" className="font-medium text-fg">
        {label}
      </span>
    );
  }
  return (
    <>
      <Link
        to="/projects/$project/models"
        params={{ project }}
        className="focus-ring touch-target rounded-sm hover:text-fg hover:underline"
      >
        {label}
      </Link>
      {model ? (
        <>
          <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />
          <span aria-current="page" className="font-mono font-medium text-fg">
            {model}
          </span>
        </>
      ) : null}
    </>
  );
}

export function Shell({
  project,
  fill = false,
  sidebar,
  children,
}: {
  project: string;
  /**
   * The page takes the whole working area (T-2908): no breadcrumb, width cap, padding or footer,
   * exactly the viewport under the header in height, and the project navigation folded behind
   * the menu button at every width so the page gets the room. The App's Open page is one.
   */
  fill?: boolean;
  /**
   * The sidebar's own navigation instead of the project's (T-4831): Administration lists its
   * sections there and shows no project switcher, since nothing on its pages belongs to a project.
   * It is handed the function that closes the drawer on a phone.
   */
  sidebar?: (closeNav: () => void) => ReactNode;
  children: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  const branding = useBranding();
  const projectTitle = useProjectTitles();
  const hiddenSections = useHiddenSections();
  const matchRoute = useSafeMatchRoute();
  // The organization's tabs, every endpoint and `/` open on this project next (T-2753).
  useEffect(() => {
    rememberProject(project);
  }, [project]);
  // The page in hand, so switching away and back reopens it (T-5603).
  const href = useRouterState({ select: (state) => state.location.href });
  useEffect(() => {
    rememberAddress(project, href);
  }, [project, href]);
  // Which page is in hand, so a panel left by a page that threw is cleared by walking away
  // from it (T-2426).
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  // The model editor lives at /models?edit=<id> or ?new=…: its crumb leads back to the list too.
  const editingModel = useRouterState({
    select: (state) => {
      const search = state.location.search as { edit?: unknown; new?: unknown };
      return typeof search.edit === "string" ? search.edit : search.new !== undefined ? "" : undefined;
    },
  });
  // The sidebar is a drawer on a phone; a navigation closes it.
  const [navOpen, setNavOpen] = useState(false);
  // Where the closed assistant's button goes on a phone; the dock renders after the header.
  const [assistantSlot, setAssistantSlot] = useState<HTMLElement | null>(null);
  // The feedback dialog, opened by its header button or, on a phone, from the Help menu (T-3319).
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  const closeNav = () => setNavOpen(false);
  const menuButton = useRef<HTMLButtonElement>(null);

  // At phone width the navigation is a sheet over the page, and the scrim that closes it is
  // `aria-hidden` and pointer-only by design — a second control in the tab order named "close
  // the menu" beside the toggle that opened it would be a duplicate. Escape is the way out for
  // the keyboard, and the focus goes back to the toggle that opened it (UI-15).
  useEffect(() => {
    if (!navOpen) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setNavOpen(false);
        menuButton.current?.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [navOpen]);

  // Approvals has its own routes, so the generic `$plural` match never fires for it.
  const approvalDetail = matchRoute({ to: "/projects/$project/approvals/$id" });
  const onApprovals = Boolean(approvalDetail || matchRoute({ to: "/projects/$project/approvals" }));
  // A copy's own pages (compare, bring back, try it) had no trail at all (T-2760): they sit under
  // Copies.
  const copyDetail =
    matchRoute({ to: "/projects/$project/workspaces/$name/compare" }) ||
    matchRoute({ to: "/projects/$project/workspaces/$name/bring-back" }) ||
    matchRoute({ to: "/projects/$project/workspaces/$name/try-it" });
  // The one thing the page is about: an approval, a copy, or an item named by the address.
  const { itemName, spacePage, creating } = addressItem(pathname);
  const detail = approvalDetail
    ? { to: "/projects/$project/approvals" as const, name: approvalDetail.id }
    : copyDetail
      ? { to: "/projects/$project/workspaces" as const, name: copyDetail.name }
      : null;
  const onAssistant = Boolean(
    matchRoute({ to: "/projects/$project/assistant", params: { project } }),
  );

  const organizationActive = Boolean(matchRoute({ to: "/organization/$tab", fuzzy: true }));
  // An Organization's own administrator opens its own sections of Administration (T-5248).
  const { opensAdministration } = useAdministers();
  const modelsActive = Boolean(matchRoute({ to: "/projects/$project/models", params: { project }, fuzzy: true }));
  const exploreActive = Boolean(matchRoute({ to: "/projects/$project/explore", params: { project } }));
  const ckanActive = Boolean(matchRoute({ to: "/projects/$project/ckan", params: { project } }));
  const knowledgeActive = Boolean(
    matchRoute({ to: "/projects/$project/knowledge", params: { project }, fuzzy: true }),
  );
  const mcpActive = Boolean(matchRoute({ to: "/projects/$project/mcp", params: { project } }));
  const catalogueActive = Boolean(matchRoute({ to: "/catalogue", fuzzy: true }));
  const importActive = Boolean(matchRoute({ to: "/projects/$project/import", params: { project } }));
  const inboxActive = Boolean(matchRoute({ to: "/projects/$project/inbox", params: { project } }));
  // The pages below the section list end the trail with their own name too (T-4216): it stopped
  // at the project, so a person on Explore or the inbox read only which project they were in.
  const otherPage = modelsActive
    ? "nav.models"
    : exploreActive
      ? "nav.explore"
      : ckanActive
        ? "nav.ckan"
        : knowledgeActive
          ? "nav.knowledge"
          : mcpActive
            ? "nav.mcp"
            : importActive
              ? "nav.import"
              : inboxActive
                ? "nav.inbox"
                : null;

  // Project settings is one entry over six tab addresses (T-2606).
  const onSettings = Boolean(
    matchRoute({ to: "/projects/$project/settings/$tab", params: { project }, fuzzy: true }),
  );

  // By the address itself: a match against the static route also answered on the other pages.
  const onHome = pathname.replace(/\/+$/, "") === `/projects/${project}/home`;
  const activeSection = NAV_SECTIONS.find((section) =>
    section.plural === "home"
      ? onHome
      : section.plural === "approvals"
      ? onApprovals
      : section.plural === "workspaces" && copyDetail
        ? true
      : section.plural === "assistant"
        ? onAssistant
        : section.plural === "settings"
          ? onSettings
          : Boolean(
            matchRoute({
              to: "/projects/$project/$plural",
              params: { project, plural: section.plural },
              // An item of the section is in the section: its edit form, its page (T-3239).
              fuzzy: true,
            }),
          ),
  );

  return (
    <div
      className={clsx(
        "flex flex-col bg-bg font-sans text-fg",
        // A page that fills the Shell gets exactly the window: the header, then everything else,
        // and nothing of the Shell's own scrolls (T-2908).
        fill ? "h-dvh overflow-hidden" : "min-h-screen",
      )}
    >
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50 focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-primary-fg"
      >
        {t("nav.skipToContent")}
      </a>

      <header className="sticky top-0 z-30 flex h-14 items-center gap-2 border-b border-border bg-surface/95 px-3 backdrop-blur sm:px-4">
        <Button
          ref={menuButton}
          variant="ghost"
          size="sm"
          className={fill ? undefined : "lg:hidden"}
          aria-label={t("nav.menu")}
          // An icon-only control says what it is to a pointer as well as to a screen reader
          // (T-1731): the glyph alone is a guess for everybody.
          title={t("nav.menu")}
          aria-expanded={navOpen}
          aria-controls="portal-sidebar"
          onClick={() => setNavOpen((open) => !open)}
          icon={<Icon name={navOpen ? "close" : "menu"} className="size-5" />}
        />
        <Link
          to="/projects/$project/$plural"
          params={{ project, plural: "spaces" }}
          // On a phone the toolbar keeps its controls and the name gives way to them: the name
          // truncates instead of pushing the menus off the side (UI-15, T-3195), down to the
          // finger's 44 px on a touch screen (`touch-target`, T-5596).
          className="focus-ring touch-target flex min-w-0 pointer-coarse:min-w-11 rounded-md text-body text-fg"
        >
          <BrandMark short />
        </Link>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {/* The closed assistant's button on a phone (T-4992); empty from `sm` up. */}
          <span ref={setAssistantSlot} className="contents" />
          {/* Administration is one entry, for organization administrators only, and its page
              holds every organization-wide power as tabs, the whole-project export and import
              included (UI-75, UI-87, owner 2026-09-25). */}
          {opensAdministration ? (
            <Link
              to="/organization/$tab"
              params={{ tab: "settings" }}
              aria-label={t("nav.organization")}
              aria-current={organizationActive ? "page" : undefined}
              className={buttonClass("ghost", "sm", "max-sm:hidden")}
            >
              <Icon name="access" className="size-4" />
              <span aria-hidden="true" className="hidden sm:inline">
                {t("nav.organization")}
              </span>
            </Link>
          ) : null}
          {/* On a phone the switcher sits atop the sidebar, where its name has room (UI-15). */}
          <span className="contents max-sm:hidden">
            <OrganizationSwitcher active={project} />
          </span>
          <LanguageSwitcher />
          <JobsMenu />
          <FeedbackButton open={feedbackOpen} onOpenChange={setFeedbackOpen} buttonClassName="hidden sm:inline-flex" />
          <HelpMenu onFeedback={() => setFeedbackOpen(true)} />
          <InboxButton project={project} />
          <UserMenu />
        </div>
      </header>

      <div className={clsx("flex flex-1", fill && "min-h-0")}>
        {navOpen ? (
          <div
            aria-hidden="true"
            onClick={closeNav}
            className={clsx("fixed inset-0 top-14 z-30 bg-overlay", !fill && "lg:hidden")}
          />
        ) : null}
        <nav
          id="portal-sidebar"
          aria-label={t("nav.main")}
          className={clsx(
            "z-40 w-sidebar shrink-0 flex-col gap-4 overflow-y-auto border-r border-border bg-surface p-3",
            // The viewport minus the `h-14` header (the below-header token).
            // From lg only: at 768 px the open sidebar left a table 460 px and cut its actions,
            // so a tablet opens it from the menu button like a phone (T-4265, photo audit T-4160 #41).
            !fill && "lg:sticky lg:top-14 lg:flex lg:h-below-header",
            navOpen ? "fixed bottom-0 left-0 top-14 flex shadow-3" : "hidden",
          )}
        >
          {sidebar ? sidebar(closeNav) : (<>
          <div className="flex flex-col gap-2">
            <span className="contents sm:hidden">
              <OrganizationSwitcher active={project} inSidebar />
            </span>
            <ProjectSelector active={project} />
            {/* Opening a project is a setting of the organization, so the control is always
                here, disabled with the reason when this caller may not (UI-44, PF-65). */}
            <NewProjectButton project={project} />
          </div>
          <ul className="flex flex-col gap-0.5">
            {NAV_SECTIONS.filter((section) => !isHiddenSection(section.plural, hiddenSections)).map((section) => {
              const isActive = section === activeSection;
              const body = <NavLabel icon={section.icon} label={t(section.labelKey)} />;
              const own = ownRoute(section.plural);
              return (
                <Fragment key={section.plural}>
                <li>
                  {own ? (
                    <Link
                      to={own}
                      params={{ project }}
                      onClick={closeNav}
                      aria-current={isActive ? "page" : undefined}
                      className={navLinkClass(isActive)}
                    >
                      {body}
                      {section.plural === "approvals" ? <PendingBadge project={project} /> : null}
                    </Link>
                  ) : (
                    <Link
                      to="/projects/$project/$plural"
                      params={{ project, plural: section.plural }}
                      onClick={closeNav}
                      aria-current={isActive ? "page" : undefined}
                      className={navLinkClass(isActive)}
                    >
                      {body}
                    </Link>
                  )}
                </li>
                {/* A model says what every space holds, so it sits right under the spaces
                    (T-6090); it is no resource list, so it has its own route. */}
                {section.plural === "spaces" ? (
                  <li>
                    <Link
                      to="/projects/$project/models"
                      params={{ project }}
                      onClick={closeNav}
                      aria-current={modelsActive ? "page" : undefined}
                      className={navLinkClass(modelsActive)}
                    >
                      <NavLabel icon="models" label={t("nav.models")} />
                    </Link>
                  </li>
                ) : null}
                </Fragment>
              );
            })}
          </ul>
          {/* Not sections of the resource API: the explorer and the CKAN view hang below the
              list. Every endpoint across projects is the Organization page's, for administrators (T-2877). */}
          <ul className="flex flex-col gap-0.5 border-t border-border pt-3">
            <li>
              <Link
                to="/projects/$project/explore"
                params={{ project }}
                onClick={closeNav}
                aria-current={exploreActive ? "page" : undefined}
                className={navLinkClass(exploreActive)}
              >
                <NavLabel icon="explore" label={t("nav.explore")} />
              </Link>
            </li>
            <li>
              <Link
                to="/projects/$project/ckan"
                params={{ project }}
                onClick={closeNav}
                aria-current={ckanActive ? "page" : undefined}
                className={navLinkClass(ckanActive)}
              >
                <NavLabel icon="ckan" label={t("nav.ckan")} />
              </Link>
            </li>
            <li>
              <Link
                to="/projects/$project/knowledge"
                params={{ project }}
                onClick={closeNav}
                aria-current={knowledgeActive ? "page" : undefined}
                className={navLinkClass(knowledgeActive)}
              >
                <NavLabel icon="search" label={t("nav.knowledge")} />
              </Link>
            </li>
            <li>
              <Link
                to="/projects/$project/mcp"
                params={{ project }}
                onClick={closeNav}
                aria-current={mcpActive ? "page" : undefined}
                className={navLinkClass(mcpActive)}
              >
                <NavLabel icon="endpoints" label={t("nav.mcp")} />
              </Link>
            </li>
            <li>
              <Link
                to="/catalogue"
                onClick={closeNav}
                aria-current={catalogueActive ? "page" : undefined}
                className={navLinkClass(catalogueActive)}
              >
                <NavLabel icon="search" label={t("nav.catalogue")} />
              </Link>
            </li>
            <li>
              <Link
                to="/projects/$project/import"
                params={{ project }}
                onClick={closeNav}
                aria-current={importActive ? "page" : undefined}
                className={navLinkClass(importActive)}
              >
                <NavLabel icon="import" label={t("nav.import")} />
              </Link>
            </li>
          </ul>
          </>)}
          {/* The signed-in person and sign-out live in the header's account menu only (owner 2026-11-01:
              "it's on two places"); the organization is one header button (T-4831). */}
        </nav>
        {fill ? (
          <main id="main" className="flex min-h-0 min-w-0 flex-1 flex-col">
            <ErrorBoundary
              resetKey={pathname}
              fallback={(reference, retry) => (
                <div className="px-4 pt-5 sm:px-gutter">
                  <PageStopped project={project} reference={reference} retry={retry} />
                </div>
              )}
            >
              <Suspense fallback={<PageLoading label={t("app.loading")} />}>{children}</Suspense>
            </ErrorBoundary>
          </main>
        ) : (
          // The footer ends the page's column and not the whole shell: under the shell it made
          // every page the window plus its height, and a scroll slid the window-tall sidebar
          // up over the header (T-4236).
          <div className="flex min-w-0 flex-1 flex-col">
          <main id="main" className="min-w-0 flex-1">
            {/* `pb-24` keeps room under the page for the assistant's floating button (56 px at
                16 px from the corner): it sat on the last column of a table at 1440 with nothing
                below it to scroll to (T-2760). */}
            <div className="mx-auto flex max-w-content flex-col gap-section px-4 pb-24 pt-5 sm:px-gutter sm:pt-6">
              <div className="flex flex-wrap items-center justify-between gap-2">
              <nav aria-label={t("nav.breadcrumb")} className="text-caption text-fg-muted">
                <ol className="flex flex-wrap items-center gap-1">
                  {/* The organization first (T-3239): its page for an administrator, its name
                      for everyone else; an installation that names none has no such crumb. */}
                  {branding.organisation ? (
                    <li className="flex items-center gap-1">
                      {opensAdministration ? (
                        <Link to="/organization/$tab" params={{ tab: "settings" }} className="focus-ring touch-target rounded-sm hover:text-fg hover:underline">
                          {branding.organisation}
                        </Link>
                      ) : (
                        <span>{branding.organisation}</span>
                      )}
                      <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />
                    </li>
                  ) : null}
                  {/* Administration is the organization's page: its trail names no project (T-4223). */}
                  {organizationActive ? (
                    <li>
                      <span aria-current="page" className="font-medium text-fg">
                        {t("nav.organization")}
                      </span>
                    </li>
                  ) : (
                  <li>
                    <Link
                      to="/projects/$project/$plural"
                      params={{ project, plural: "spaces" }}
                      className="focus-ring touch-target rounded-sm hover:text-fg hover:underline"
                    >
                      {projectTitle(project)}
                    </Link>
                  </li>
                  )}
                  {activeSection ? (
                    <li className="flex items-center gap-1">
                      <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />
                      <SectionCrumb
                        project={project}
                        plural={activeSection.plural}
                        label={t(activeSection.labelKey)}
                        detail={detail}
                        spacePage={spacePage}
                        itemName={itemName}
                        creating={creating}
                      />
                    </li>
                  ) : null}
                  {!activeSection && otherPage ? (
                    <li className="flex items-center gap-1">
                      <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />
                      <OtherPageCrumb
                        project={project}
                        label={t(otherPage)}
                        model={crumbModel(modelsActive, itemName, editingModel)}
                      />
                    </li>
                  ) : null}
                </ol>
              </nav>
              {/* The page's star and its link to send (UI-90, UI-91). */}
              <PageTools />
              </div>
              <WorkspaceBar project={project} />
              {/* The page, and the panel that takes its place when it throws. `resetKey` is the
                  path, so walking away from a failed page with the sidebar that is still there
                  shows the next page rather than the panel again (T-2426). */}
              <ErrorBoundary
                resetKey={pathname}
                fallback={(reference, retry) => (
                  <PageStopped project={project} reference={reference} retry={retry} />
                )}
              >
                <Suspense fallback={<PageLoading label={t("app.loading")} />}>{children}</Suspense>
              </ErrorBoundary>
            </div>
          </main>
          <ShellFooter organisation={branding.organisation} contactEmail={branding.contactEmail} />
          </div>
        )}
        {/* The assistant is on the right of every page a run is remembered: a column beside
            the page, or a bubble at the bottom right when hidden (UI-45). */}
        <AssistantDock project={project} launcherSlot={assistantSlot} />
        {/* A tab connected to a connector says so on every page, with its disconnect (T-5967). */}
        <PairedTabBar />
        <CommandPalette project={project} />
      </div>

    </div>
  );
}
