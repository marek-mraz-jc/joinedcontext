import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useAuth } from "../../auth/AuthProvider";
import { useBranding } from "../../branding";
import {
  Button,
  buttonClass,
  EmptyState,
  ExternalLink,
  Icon,
  InlineError,
  PageHeader,
  PageLoading,
  safeHref,
} from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { useAppBuild } from "./AppBuildPanel";
import { appSpec, openBlockedReason } from "./AppsCatalog";
import { appDisplayName } from "./appTitle";

/**
 * The App's own address: its own host `{name}.apps.{domain}` under the apex the Portal serves
 * Apps from when it has one (`appsOrigin` of the branding, from `JC_PORTAL_APPS_URL`; AP-133),
 * else the path on the Portal's own host.
 */
export function appAddress(name: string, appsOrigin?: string | null): string {
  const path = `/apps/${encodeURIComponent(name)}/`;
  // Only a DNS label is a host's first label; anything else keeps the path, which the Portal
  // answers with a 404.
  if (!appsOrigin || !/^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(name)) return path;
  try {
    const url = new URL("/", appsOrigin);
    if (url.protocol !== "https:" && url.protocol !== "http:") return path;
    url.hostname = `${name}.apps.${url.hostname}`;
    return url.toString();
  } catch {
    return path;
  }
}

/**
 * The App's address in the Portal's language (T-4264, T-4160 #39): the SDK reads `?lang=` before
 * the browser's languages (T-4199), so an App framed in the Slovak Portal answers in Slovak.
 */
export function inLanguage(address: string, language: string | undefined): string {
  const lang = language?.split("-")[0].toLowerCase();
  if (!lang || !/^[a-z]{2,3}$/.test(lang)) return address;
  const [path, hash = ""] = address.split("#", 2);
  const separator = path.includes("?") ? "&" : "?";
  return `${path}${separator}lang=${lang}${hash ? `#${hash}` : ""}`;
}

const FRAME_SANDBOX = "allow-scripts allow-forms allow-popups allow-downloads";

/**
 * What the App's frame may do (AP-122, AP-19): run its scripts, send its forms, open a link in a
 * new window, download, and never take the Portal's window away. It keeps its own origin
 * (`allow-same-origin`) only when its address is on another origin than the Portal's: there the
 * pair gives the App its session and storage as in a window of its own, while on the Portal's
 * origin the pair would hand it the Portal's CSRF cookie and is no sandbox at all.
 */
export function appFrameSandbox(src: string, portalOrigin: string = window.location.origin): string {
  return appFrameOrigin(src, portalOrigin) === "null" ? FRAME_SANDBOX : `${FRAME_SANDBOX} allow-same-origin`;
}

/**
 * The origin the App's framed document has, and so the one its messages carry: its own when it
 * keeps it (another origin than the Portal's), else the opaque `"null"` of a sandbox without
 * `allow-same-origin`.
 */
export function appFrameOrigin(src: string, portalOrigin: string = window.location.origin): string {
  try {
    const origin = new URL(src, portalOrigin).origin;
    return origin !== portalOrigin ? origin : "null";
  } catch {
    return "null";
  }
}

/** How long the Open page waits after the frame's load for the App to say it is up (T-2941). */
export const FRAME_ANSWER_MS = 8000;

/** How long the silent sign-in check may take before its answer counts as unreadable (T-3034). */
export const SSO_CHECK_MS = 6000;

/** The Portal's silent sign-in check: the realm with `prompt=none`, landing on the page below. */
export const SSO_CHECK_PATH = "/api/v1/auth/sso-check";
const SSO_CHECK_DONE = "/api/v1/auth/sso-check/done";

/** The realm's answers that mean the person has to sign in where the realm may show its form. */
const SIGN_IN_NEEDED = new Set(["login_required", "interaction_required", "consent_required", "account_selection_required"]);

/**
 * What the realm answered the silent check (AP-122, T-3034), read off the address its frame landed
 * on: `null` while it has not landed on the Portal's page, `"unknown"` for an answer that says
 * neither. Only the error decides; a code, when the session lives, is never read out.
 */
export function ssoAnswer(href: string | undefined): "signedIn" | "signedOut" | "unknown" | null {
  if (!href) return null;
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  if (url.origin !== window.location.origin || url.pathname !== SSO_CHECK_DONE) return null;
  const answer = new URLSearchParams(url.hash.slice(1));
  const error = answer.get("error");
  if (error) return SIGN_IN_NEEDED.has(error) ? "signedOut" : "unknown";
  return answer.has("code") ? "signedIn" : "unknown";
}

/** The address a frame of the Portal's own origin is on; `undefined` while it is on another. */
function frameAddress(frame: HTMLIFrameElement): string | undefined {
  try {
    return frame.contentWindow?.location.href;
  } catch {
    return undefined;
  }
}

/**
 * Whether the framed App needs its person to sign in again (AP-122, T-2941, T-3034). An App built
 * with the SDK posts `{kind: "jc-ready"}` once it is mounted, and then nothing is asked. A silent
 * App proves nothing: one built before the SDK sent the message, or without the SDK, renders all
 * the same. So silence starts a silent check of the realm session in a hidden frame of the
 * Portal's own origin, and only a realm that says the person must sign in brings the offer; a
 * live session, an answer that cannot be read, and a check that does not land in time do not.
 * Only a `jc-ready` from this frame's own window and the App's origin counts.
 */
function useSignInNeeded(address: string) {
  const frame = useRef<HTMLIFrameElement>(null);
  const answered = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const checkTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // The address each state was seen on: a new address is a new visit.
  const [checkingOn, setCheckingOn] = useState<string | null>(null);
  const [signedOutOn, setSignedOutOn] = useState<string | null>(null);
  useEffect(() => {
    answered.current = false;
    const expected = appFrameOrigin(address);
    const listen = (event: MessageEvent) => {
      const own = frame.current?.contentWindow;
      if (!own || event.source !== own || event.origin !== expected) return;
      const data: unknown = event.data;
      if (typeof data !== "object" || data === null || (data as { kind?: unknown }).kind !== "jc-ready") return;
      answered.current = true;
      clearTimeout(timer.current);
      clearTimeout(checkTimer.current);
      setCheckingOn(null);
      setSignedOutOn(null);
    };
    window.addEventListener("message", listen);
    return () => {
      window.removeEventListener("message", listen);
      clearTimeout(timer.current);
      clearTimeout(checkTimer.current);
    };
  }, [address]);
  // The App's script may answer before its document's `load`; an answer counts for the whole visit.
  const onLoad = () => {
    clearTimeout(timer.current);
    if (answered.current) return;
    timer.current = setTimeout(() => {
      if (answered.current) return;
      setCheckingOn(address);
      clearTimeout(checkTimer.current);
      checkTimer.current = setTimeout(() => setCheckingOn(null), SSO_CHECK_MS);
    }, FRAME_ANSWER_MS);
  };
  // Every load of the check frame is read; a load on the realm's side (another origin) is not
  // an answer, and the timer ends a check that never lands.
  const onCheckLoad = (event: { currentTarget: HTMLIFrameElement }) => {
    const answer = ssoAnswer(frameAddress(event.currentTarget));
    if (answer === null) return;
    clearTimeout(checkTimer.current);
    setCheckingOn(null);
    if (answer === "signedOut" && !answered.current) setSignedOutOn(address);
  };
  return [
    frame,
    {
      signedOut: signedOutOn === address,
      checking: checkingOn === address,
      onLoad,
      onCheckLoad,
      dismiss: () => setSignedOutOn(null),
    },
  ] as const;
}

/** The one App manifest, shared with every page that reads it. */
export function useApp(project: string, name: string) {
  return useQuery({
    queryKey: queryKeys.resource(project, "apps", name),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "apps", name } },
        }),
      ) as unknown as Manifest,
  });
}

/**
 * "Open app" on the App's own page (AP-122): the in-Portal page when something is served, the
 * reason in place of the button when not. A retired App offers nothing to open.
 */
export function OpenAppButton({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t } = useTranslation();
  const app = useApp(project, name);
  const build = useAppBuild(project, name);
  // Nothing until the App is read: no guessed button, and no button for a name that is no App.
  if (app.data?.kind !== "App") return null;
  if (appSpec(app.data).lifecycle === "retired") return null;
  const reason = openBlockedReason(app.data, build.data?.run ?? null, t);
  return reason ? (
    <Button size="sm" variant="primary" disabled disabledReason={reason}>
      {t("apps.openAction")}
    </Button>
  ) : (
    <Link
      to="/projects/$project/$plural/$name/open"
      params={{ project, plural: "apps", name }}
      className={buttonClass("primary", "sm")}
    >
      {t("apps.openAction")}
    </Link>
  );
}

/**
 * The frame's own fullscreen (T-2908): the container asks the browser, Escape or the same button
 * gives the window back, and the state follows the document, so an Escape the browser handles
 * itself is not missed. Where the browser offers no fullscreen the control says so.
 */
function useFullscreen() {
  const target = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(false);
  useEffect(() => {
    const follow = () => setActive(document.fullscreenElement !== null && document.fullscreenElement === target.current);
    document.addEventListener("fullscreenchange", follow);
    return () => document.removeEventListener("fullscreenchange", follow);
  }, []);
  const toggle = async () => {
    if (document.fullscreenElement) {
      await document.exitFullscreen();
    } else {
      await target.current?.requestFullscreen();
    }
  };
  return [target, { active, available: document.fullscreenEnabled === true, toggle }] as const;
}

/**
 * A published App inside the Portal (AP-122, UI-44). The page is a slim bar (the App's name and
 * its actions) over a frame that takes everything else the window has: the Shell renders it with
 * `fill`, so nothing below the bar scrolls but the App itself (T-2908). The frame is sandboxed
 * and only the Portal's host may frame an App. "Open in new window" opens the same address as a
 * page of its own. A retired, unpublished or never-built App shows its state instead of a frame
 * that would only say "not found".
 */
export function AppOpenPage({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const { signIn } = useAuth();
  const { appsOrigin } = useBranding();
  const app = useApp(project, name);
  const build = useAppBuild(project, name);
  const [frameArea, fullscreen] = useFullscreen();
  const [fullscreenFailed, setFullscreenFailed] = useState<string | null>(null);
  const address = inLanguage(appAddress(name, appsOrigin), i18n.language);
  const [frame, session] = useSignInNeeded(address);
  const signInAgain = () => {
    signIn(`${window.location.pathname}${window.location.search}`);
  };

  const toApp = () => {
    void navigate({
      to: "/projects/$project/$plural/$name",
      params: { project, plural: "apps", name },
    });
  };
  const details = <Button onClick={toApp}>{t("apps.openPage.details")}</Button>;
  const fallbackTitle = appDisplayName({ appName: name });

  if (app.isPending) {
    return (
      <div className="space-y-4 px-4 pt-5 sm:px-gutter">
        <PageHeader title={fallbackTitle} description={t("apps.openPage.lead")} actions={details} />
        <PageLoading label={t("app.loading")} />
      </div>
    );
  }
  if (app.isError) {
    return (
      <div className="px-4 pt-5 sm:px-gutter">
      <ResourcePageFailed
        title={fallbackTitle}
        description={t("apps.openPage.lead")}
        error={app.error}
        onRetry={() => {
          void app.refetch();
        }}
        back={
          <Button onClick={() => void navigate({ to: "/projects/$project/$plural", params: { project, plural: "apps" } })}>
            {t("apps.back")}
          </Button>
        }
      />
      </div>
    );
  }

  const manifest = app.data;
  const title = localized(manifest.metadata.title, i18n.language, fallbackTitle);
  const blocked = openBlockedReason(manifest, build.data?.run ?? null, t);

  if (blocked) {
    return (
      <div className="space-y-4 px-4 pt-5 sm:px-gutter">
        <PageHeader title={title} description={t("apps.openPage.lead")} actions={details} />
        <EmptyState title={blocked} description={t("apps.openPage.notOpen")} icon="apps" />
      </div>
    );
  }

  const sourceCode = safeHref(build.data?.repositoryUrl);
  const configuration = safeHref(build.data?.configurationUrl);

  // The commit the App is served from is on its details page, in the build badge's tooltip:
  // developer information has no line of the person's screen here (owner, 2026-09-25).
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-surface px-3 py-1.5 sm:px-4">
        <h1 className="min-w-0 flex-1 truncate text-body font-semibold text-fg">{title}</h1>
        <div className="flex flex-wrap items-center gap-1.5">
          <Button size="sm" variant="ghost" onClick={toApp} icon={<Icon name="chevronLeft" className="size-4" />}>
            {t("apps.openPage.details")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={fullscreen.active}
            disabled={!fullscreen.available}
            disabledReason={fullscreen.available ? undefined : t("apps.openPage.fullscreenUnavailable")}
            onClick={() => {
              setFullscreenFailed(null);
              fullscreen.toggle().catch((error: unknown) => {
                setFullscreenFailed(error instanceof Error ? error.message : String(error));
              });
            }}
          >
            {fullscreen.active ? t("apps.openPage.fullscreenExit") : t("apps.openPage.fullscreen")}
          </Button>
          <ExternalLink href={address} hideIcon className={buttonClass("ghost", "sm")}>
            {t("apps.openPage.newWindow")}
          </ExternalLink>
          {/* The App's whole source and the project's configuration on the forge, each only when
              the API gives this person its address (AP-103, T-3039). */}
          {sourceCode ? (
            <ExternalLink href={sourceCode} hideIcon className={buttonClass("ghost", "sm")}>
              {t("apps.sourceCode")}
            </ExternalLink>
          ) : null}
          {configuration ? (
            <ExternalLink href={configuration} hideIcon className={buttonClass("ghost", "sm")}>
              {t("apps.projectConfiguration")}
            </ExternalLink>
          ) : null}
          {/* An App whose session ran out keeps asking inside the frame, where Keycloak may not
              be framed: the way back is the Portal's own sign-in, in the top window (AP-122). */}
          <Button
            size="sm"
            variant="ghost"
            title={t("apps.openPage.signInHint")}
            aria-describedby="app-open-sign-in-hint"
            onClick={signInAgain}
          >
            {t("apps.openPage.signInAgain")}
          </Button>
          <span id="app-open-sign-in-hint" className="sr-only">
            {t("apps.openPage.signInHint")}
          </span>
        </div>
      </div>
      {fullscreenFailed ? (
        <InlineError className="border-b border-border bg-surface px-3 py-1.5 sm:px-4">
          {t("apps.openPage.fullscreenFailed", { reason: fullscreenFailed })}
        </InlineError>
      ) : null}
      {/* Above the frame, never over it: the App stays where it is while the person signs in. */}
      <div role="status" aria-live="polite">
        {session.signedOut ? (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-border bg-surface-muted px-3 py-2 sm:px-4">
            <p className="min-w-0 flex-1 text-caption text-fg">
              <span className="font-semibold">{t("apps.openPage.signedOutTitle")}</span> {t("apps.openPage.signedOutBody")}
            </p>
            <div className="flex flex-wrap items-center gap-1.5">
              <Button size="sm" variant="primary" onClick={signInAgain}>
                {t("apps.openPage.signInAgain")}
              </Button>
              {/* "Open in new window" is the bar's own link just above: one control, one name. */}
              <Button size="sm" variant="ghost" onClick={session.dismiss}>
                {t("apps.openPage.signedOutDismiss")}
              </Button>
            </div>
          </div>
        ) : null}
      </div>
      <div ref={frameArea} data-testid="app-frame-area" className="flex min-h-0 flex-1 flex-col bg-surface">
        {/* The silent sign-in check (T-3034): the Portal's own origin, no script, gone once read. */}
        {session.checking ? (
          <iframe
            hidden
            aria-hidden="true"
            tabIndex={-1}
            title={t("apps.openPage.signedOutTitle")}
            data-testid="app-sso-check"
            src={SSO_CHECK_PATH}
            sandbox="allow-same-origin"
            referrerPolicy="no-referrer"
            onLoad={session.onCheckLoad}
          />
        ) : null}
        <iframe
          ref={frame}
          onLoad={session.onLoad}
          src={address}
          title={t("apps.openPage.frameTitle", { title })}
          sandbox={appFrameSandbox(address)}
          referrerPolicy="no-referrer"
          className="block min-h-0 w-full flex-1 border-0"
        />
      </div>
    </div>
  );
}
