import { Suspense, lazy, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Alert, Button, Card, Field, PermissionGuard, Textarea } from "../components/ui";
import type { JSX, ReactNode } from "react";
import { createPortal } from "react-dom";
import { clsx } from "clsx";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ASSISTANT_EVENT } from "../navigation/shortcuts";
import { useIsPhone } from "../phone";
import { useHiddenSections } from "../branding";
import { isHiddenSection } from "../components/layout/navigation";
import { api, ApiError, unwrap } from "../api/client";
import { usePermissions } from "../api/permissions";
import { ConversationPanel } from "../pages/apps/ConversationPanel";
import { TERMINAL_STATES, useAgentRun } from "../pages/apps/useAgentRun";
import type { RunEvent } from "../pages/apps/useAgentRun";
import { appDisplayName, useEndpointTitles } from "../pages/apps/appTitle";
import {
  DataBar,
  endpointsQuery,
  MAX_ENDPOINTS,
  rememberEndpoints,
  runEndpointNames,
  sameEndpoints,
  storedEndpoints,
} from "./EndpointPicker";
import {
  CapabilitiesControl,
  PRESET_PATHS,
  accessFor,
  rememberCapabilities,
  storedCapabilities,
} from "./Capabilities";
import type { Capabilities } from "./Capabilities";
import { Icon } from "../components/ui/icons";
import { pageOf } from "./pageOf";
import { ConversationList } from "./ConversationList";
import type { IconName } from "../components/ui/icons";
import {
  dismissNotice,
  isPortalRoute,
  navigatedSeq,
  noticeSnapshot,
  onAssistantChange,
  actedSeq,
  onAskRequest,
  onPickChange,
  formContext,
  onOpenRequest,
  pageContext,
  parseRun,
  pickElement,
  pickedElement,
  rememberActed,
  rememberNavigated,
  rememberPrefill,
  rememberRun,
  runSnapshot,
  settleNotice,
  settlePrefill,
  trail,
} from "./state";
import { runUiAction, uiActionOf } from "./uiActions";

// The model parser and the app builder load when the dock shows them, not with every page (T-3316).
const ModelFileDrop = lazy(() => import("../pages/models/ModelFileDrop").then((m) => ({ default: m.ModelFileDrop })));
const CompleteFlow = lazy(() => import("../pages/spaces/SpaceComplete").then((m) => ({ default: m.CompleteFlow })));
const AppGenerator = lazy(() => import("../pages/apps/AppGenerator").then((m) => ({ default: m.AppGenerator })));

/**
 * The assistant, on the right of every page (UI-45, UI-51, UI-52, UI-53).
 *
 * Renders as a round bubble at the bottom right whenever the panel is closed. When open,
 * renders a 24 rem right-docked panel (full width on small viewports, full screen on toggle).
 * If no run is remembered, shows the empty state with the paths and a paperclip in the
 * composer that drafts a data model from a sample file. The composer is the last element of the
 * dock in both states and does not move: what is above it scrolls (T-2424).
 * When a run is remembered, connects the live conversation panel. Open, it sits beside the page,
 * floats over it, or fills the screen; the choice lasts for the tab.
 */
/** A stable array between renders: `useSyncExternalStore` compares by identity. */
let trailCache: string[] = [];
function trailSnapshot(): string[] {
  const next = trail();
  if (next.length !== trailCache.length || next.some((route, i) => route !== trailCache[i])) {
    trailCache = next;
  }
  return trailCache;
}

/**
 * The assistant's paths (ADR-N-032, AG-87): the icon each shows and the kind it ends by
 * proposing, which a role must be allowed to propose to take it. Finding data proposes nothing.
 */
const PATHS = [
  ["integrate-pipeline", "pipelines", "Pipeline"],
  ["upload-data", "import", "ContextSpace"],
  ["find-data", "search", null],
  ["share-data", "share", "Endpoint"],
  ["build-app", "apps", "App"],
  ["build-dashboard", "dashboards", "Dashboard"],
  ["create-data-model", "models", "DataModel"],
  ["define-kpi", "explore", "Pipeline"],
] as const satisfies readonly (readonly [string, IconName, string | null])[];

type PathId = (typeof PATHS)[number][0];

/** The section a path works in, where an installation may hide that section (T-2874). */
const PATH_SECTION: Partial<Record<PathId, string>> = { "build-dashboard": "dashboards" };

/**
 * The paths, as the empty assistant offers them (T-2692, UI-45).
 *
 * An empty-state affordance and nothing more: they stand while no conversation is open and are
 * gone the moment one starts (T-2464); "New conversation" brings them back. A path a role cannot
 * take stays and is disabled with its reason, which is `PermissionGuard`'s job through the shared
 * Button (T-1390, UI-44). A click starts the conversation on the path, with nothing typed: the
 * Portal asks the path's first question itself (AG-91).
 */
function Paths({
  project,
  disabled,
  capabilities,
  onPick,
}: {
  project: string;
  disabled: boolean;
  /** A path the chosen capabilities leave out is disabled with why (AG-92). */
  capabilities: Capabilities | null;
  onPick: (path: PathId) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const hiddenSections = useHiddenSections();
  // A hidden section's path is not offered at all: disabling it would name a page nobody sees.
  const offered = PATHS.filter(([path]) => {
    const section = PATH_SECTION[path];
    return section === undefined || !isHiddenSection(section, hiddenSections);
  });
  // The reason a greyed path carries is a tooltip; one visible line says it for all of them (T-4263).
  const permissions = usePermissions(project);
  // Propose on any kind opens a conversation (AG-67); a read-only role gets the reason once.
  const mayAsk = permissions.can("*", "propose");
  const denied =
    mayAsk && permissions.data !== undefined && offered.some(([, , kind]) => kind !== null && !permissions.can(kind, "propose"));
  return (
    <>
      {denied ? (
        <p data-testid="assistant-paths-denied" className="text-caption text-fg-muted">
          {t("assistant.pathsDenied")}
        </p>
      ) : null}
      <ul data-testid="assistant-paths" className="flex flex-col gap-2">
        {offered.map(([path, icon, kind]) => {
          // The shared Button, not a hand-made one: `PermissionGuard` hands it the reason through
          // `disabledReason`, which only that control knows what to do with.
          const left = capabilities !== null && !PRESET_PATHS[capabilities.preset].includes(path);
          const button = (
            <Button
              size="sm"
              disabled={disabled || left || !mayAsk}
              disabledReason={
                !mayAsk
                  ? t("assistant.readOnly")
                  : left
                    ? t("assistant.capabilities.pathLeft", {
                        preset: t(`assistant.capabilities.presets.${capabilities.preset}.title`),
                      })
                    : undefined
              }
              data-path={path}
              // The name is the title and its line, read with a pause between them; the
              // description stays free for the reason a role may not take the path.
              aria-labelledby={`assistant-path-${path} assistant-path-${path}-line`}
              onClick={() => {
                onPick(path);
              }}
              // h-auto! beats the size's h-control, else the two lines overflow the fixed height (owner 2026-10-30).
              className="h-auto! w-full items-start justify-start gap-2 whitespace-normal rounded-md bg-surface-subtle p-2 text-left"
            >
              <Icon name={icon} className="mt-0.5 size-4 shrink-0" />
              <span className="flex flex-col">
                <span id={`assistant-path-${path}`} className="text-body font-medium">
                  {t(`assistant.paths.${path}.title`)}
                </span>
                <span id={`assistant-path-${path}-line`} className="text-caption text-fg-muted">
                  {t(`assistant.paths.${path}.line`)}
                </span>
              </span>
            </Button>
          );
          return (
            <li key={path}>
              {kind ? (
                <PermissionGuard project={project} kind={kind} verb="propose">
                  {button}
                </PermissionGuard>
              ) : (
                button
              )}
            </li>
          );
        })}
      </ul>
    </>
  );
}

export function AssistantDock({
  project,
  launcherSlot = null,
}: {
  project: string;
  /** The header's place for the closed assistant's button on a phone (T-4992). */
  launcherSlot?: HTMLElement | null;
}): JSX.Element | null {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const raw = useSyncExternalStore(onAssistantChange, runSnapshot);
  const run = useMemo(() => parseRun(raw), [raw]);
  // The element the person pointed at in this run's preview: the next message edits its file only (SDK-46).
  const picked = useSyncExternalStore(onPickChange, () => pickedElement(run?.runId));
  const activeProject = run?.project ?? project;
  const navigated = useSyncExternalStore(onAssistantChange, noticeSnapshot);
  // The pages the assistant opened, so the person walks back without losing the conversation.
  const opened = useSyncExternalStore(onAssistantChange, trailSnapshot);
  // The notice goes once the person moves on from the page it names.
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  useEffect(() => {
    settleNotice(pathname);
    settlePrefill(pathname);
  }, [navigated, pathname]);
  const [open, setOpen] = useState(() => Boolean(parseRun(runSnapshot())));
  const phone = useIsPhone();
  // Opened by the person (the bubble, Ctrl/Cmd+K), the panel lands on its text box, ready to
  // type; opened because a run was remembered, it leaves the focus where it was (T-2719).
  const focusBox = useRef(false);
  useEffect(() => {
    if (open && focusBox.current) {
      focusBox.current = false;
      document.querySelector<HTMLElement>("#assistant-empty-composer, #run-message")?.focus();
    }
  });
  // The endpoints the data bar offers are read while the page is idle, so opening does not wait.
  const queryClient = useQueryClient();
  const mayAsk = usePermissions(activeProject).can("*", "propose");
  useEffect(() => {
    const read = () => {
      void queryClient.prefetchQuery(endpointsQuery(project));
    };
    const handle = window.setTimeout(read, 1000);
    return () => {
      window.clearTimeout(handle);
    };
  }, [project, queryClient]);
  const [building, setBuilding] = useState(false);
  // The table the person attached: the dock shows its way into a space until they go back.
  const [table, setTable] = useState<File | null>(null);
  const [layout, setLayout] = useState<Layout>(storedLayout);
  useEffect(() => {
    try {
      sessionStorage.setItem(LAYOUT_KEY, layout);
    } catch {
      // No storage (a private window): the layout lasts until the next page.
    }
  }, [layout]);
  const full = layout === "full";
  const wide = useSyncExternalStore(onWideChange, isWide, () => false);
  const [listOpen, setListOpen] = useState(true);
  const [panelOpen, setPanelOpen] = useState(true);
  // A run remembered after mount (the run page, the Assistant page, a started conversation)
  // opens the panel; closing forgets the run and leaves the bubble.
  const runId = run?.runId ?? null;
  const [shownRun, setShownRun] = useState(runId);
  // A change made in a running conversation's data bar waits for its next message (AG-75).
  const [pendingEndpoints, setPendingEndpoints] = useState<string[] | null>(null);
  if (runId !== shownRun) {
    setShownRun(runId);
    setPendingEndpoints(null);
    if (runId !== null) {
      setOpen(true);
    }
  }

  const [composerMessage, setComposerMessage] = useState("");
  // The palette's second Ctrl/Cmd+K, or its "Ask the assistant", opens the assistant with what
  // was typed in the composer (UI-88). Nothing is sent: the person still presses Send.
  useEffect(() => {
    const onAsk = (event: Event) => {
      const text = (event as CustomEvent<{ text?: string }>).detail?.text ?? "";
      if (text !== "") setComposerMessage((was) => (was.trim() === "" ? text : was));
      focusBox.current = true;
      setOpen(true);
    };
    window.addEventListener(ASSISTANT_EVENT, onAsk);
    return () => {
      window.removeEventListener(ASSISTANT_EVENT, onAsk);
    };
  }, []);
  const [startError, setStartError] = useState<string | null>(null);
  const [isStarting, setIsStarting] = useState(false);
  // The endpoints the next conversation may query (AG-75), remembered per project for the tab.
  const [chosenEndpoints, setChosenEndpoints] = useState<string[]>(() => storedEndpoints(project));
  const [endpointsOf, setEndpointsOf] = useState(activeProject);
  if (endpointsOf !== activeProject) {
    setEndpointsOf(activeProject);
    setChosenEndpoints(storedEndpoints(activeProject));
  }
  const chooseEndpoints = (names: string[]) => {
    setChosenEndpoints(names);
    rememberEndpoints(activeProject, names);
  };
  // "Ask the assistant" on an Endpoint's page scopes the assistant to that Endpoint (T-4760): the
  // next conversation, or the running one's next message, which then reads that Endpoint's card.
  // A run of another project keeps its own Endpoints: the name is the page project's.
  const runProject = run?.project ?? null;
  useEffect(() => {
    const onAsk = (event: Event) => {
      const asked = (event as CustomEvent<{ endpoints?: unknown }>).detail?.endpoints;
      if (!Array.isArray(asked)) return;
      const names = asked.filter((name): name is string => typeof name === "string").slice(0, MAX_ENDPOINTS);
      if (names.length === 0) return;
      if (runProject === null) {
        setChosenEndpoints(names);
        rememberEndpoints(project, names);
      } else if (runProject === project) {
        setPendingEndpoints(names);
      }
    };
    window.addEventListener(ASSISTANT_EVENT, onAsk);
    return () => {
      window.removeEventListener(ASSISTANT_EVENT, onAsk);
    };
  }, [project, runProject]);
  // What the assistant may do (AG-92, T-2718), remembered per project like the endpoints.
  const [capabilities, setCapabilities] = useState<Capabilities | null>(() => storedCapabilities(project));
  const [capabilitiesOf, setCapabilitiesOf] = useState(activeProject);
  if (capabilitiesOf !== activeProject) {
    setCapabilitiesOf(activeProject);
    setCapabilities(storedCapabilities(activeProject));
  }
  const chooseCapabilities = (chosen: Capabilities) => {
    setCapabilities(chosen);
    rememberCapabilities(activeProject, chosen);
  };

  const { run: record, events, streaming, answer, send, cancel } = useAgentRun(
    activeProject,
    run?.runId ?? null,
  );

  useEffect(() => {
    return onOpenRequest((intent) => {
      setOpen(true);
      setBuilding(intent === "build");
    });
  }, []);

  // A question written by a form's own action (T-1611): it lands in the composer and the person
  // sends it. The dock never asks on the person's behalf (AG-73).
  useEffect(() => {
    return onAskRequest((question) => {
      setComposerMessage(question);
      setBuilding(false);
    });
  }, []);

  // Escape from inside the panel hides it, as its minimize button does, and the bubble takes the
  // focus back (UI-92); full screen first goes back to the side.
  useEffect(() => {
    if (!open || full) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      // A menu, a list or a picker inside the panel takes its own Escape first and marks it handled.
      if (event.key === "Escape" && !event.defaultPrevented && event.target instanceof Element && event.target.closest("aside[data-layout]")) {
        setOpen(false);
        window.setTimeout(() => document.querySelector<HTMLElement>("[data-assistant-bubble]")?.focus(), 0);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [open, full]);

  useEffect(() => {
    if (!full) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setLayout("side");
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [full]);

  useEffect(() => {
    if (runId === null) {
      return;
    }
    const followed = navigatedSeq(runId);
    const next = events.filter((event) => event.kind === "navigate" && event.seq > followed).at(-1);
    if (!next) {
      return;
    }
    rememberNavigated(runId, next.seq);
    const route = routeOf(next);
    if (route === null) {
      return;
    }
    const draft = next.payload.draft as { kind?: string; name?: string } | undefined;
    const targetRoute =
      draft?.name && typeof draft.name === "string"
        ? `${route}${route.includes("?") ? "&" : "?"}draft=${encodeURIComponent(draft.name)}`
        : route;
    rememberPrefill(route, prefillOf(next));
    // An `href` is parsed into the path and the search: a query inside `to` reaches the address
    // bar but not the router's search, so a page already open is not remounted for it (T-0770).
    void navigate({ href: targetRoute });
  }, [events, navigate, runId]);

  // The browser actions of the run (T-5959): each runs once, in order, after the navigation
  // before it has mounted its page; a remount replays the stream and skips what already ran.
  useEffect(() => {
    if (runId === null) {
      return;
    }
    const pending = events.filter((event) => event.kind === "ui" && event.seq > actedSeq(runId));
    if (pending.length === 0) {
      return;
    }
    const last = pending[pending.length - 1];
    rememberActed(runId, last.seq);
    void (async () => {
      for (const event of pending) {
        const action = uiActionOf(event.payload);
        if (action) {
          await runUiAction(action);
        }
      }
    })();
  }, [events, runId]);

  const startConversation = async (promptText: string, path?: PathId) => {
    setStartError(null);
    setIsStarting(true);
    try {
      const created = await unwrap(
        await api.POST("/api/v1/projects/{project}/assistant/conversations", {
          params: { path: { project: activeProject } },
          // `endpointNames` is AG-75, `formContext` is T-1611.
          body: {
            message: promptText,
            endpointNames: chosenEndpoints,
            access: accessFor(capabilities, chosenEndpoints),
            formContext: formContext(),
            pageContext: pageContext(activeProject),
            path,
          },
        }),
      );
      rememberRun({ project: activeProject, runId: created.id });
      setComposerMessage("");
    } catch (err) {
      const detail =
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : String(err);
      setStartError(detail);
    } finally {
      setIsStarting(false);
    }
  };

  const over = record.data ? TERMINAL_STATES.includes(record.data.status) : false;
  const endpointTitles = useEndpointTitles(activeProject);
  // A run that builds an application says which one, by its title, never by its id.
  const buildingApp = record.data?.appName
    ? appDisplayName({
        title: record.data.title,
        appName: record.data.appName,
        endpointTitle: endpointTitles.get(record.data.endpointName),
      })
    : "";
  // Leaves this conversation for an empty one, whatever state its run is in: a run that hung
  // or died used to hold the dock, and the owner had no way back to a working chat (T-2463).
  // A run that is still going is stopped rather than left reading an inbox nobody writes to.
  const newConversation = (): void => {
    if (run && !over) {
      cancel.mutate(run.runId);
    }
    setStartError(null);
    rememberRun(null);
  };
  // The newest question the person asked, asked again in a fresh conversation (T-2462).
  const lastQuestion = [...events]
    .reverse()
    .find(
      (event) =>
        event.kind === "message" &&
        event.payload.sentBy !== "agent" &&
        typeof event.payload.text === "string",
    )
    ?.payload.text as string | undefined;
  const retry =
    lastQuestion !== undefined && lastQuestion.trim() !== ""
      ? () => {
          newConversation();
          void startConversation(lastQuestion);
        }
      : undefined;
  const lastEvent = events.length > 0 ? events[events.length - 1] : undefined;
  const isBusy = Boolean(run && !over && lastEvent && lastEvent.kind === "message");

  // The header's icon buttons: the shared ghost Button, square, quiet until hovered or pressed.
  const iconButton = "w-8 px-0 text-fg-muted hover:text-fg aria-pressed:bg-primary-soft aria-pressed:text-primary-soft-fg";

  const attach = (
    <Suspense fallback={null}>
      <ModelFileDrop
        icon
        project={activeProject}
        onPopulate={(source) => {
          rememberPrefill(`/projects/${activeProject}/models`, { source });
          void navigate({ to: "/projects/$project/models", params: { project: activeProject } });
        }}
        // A table takes the one flow that also loads its rows, here in the dock (T-6456).
        onTable={setTable}
      />
    </Suspense>
  );

  // What the running conversation queries: the newest `endpoints` event, else the run record.
  const lastEndpointsEvent = [...events].reverse().find((event) => event.kind === "endpoints");
  const runEndpoints = lastEndpointsEvent
    ? runEndpointNames(
        (Array.isArray(lastEndpointsEvent.payload.names) ? lastEndpointsEvent.payload.names : []).map(
          (name: unknown) => ({ name }),
        ),
      )
    : runEndpointNames((record.data as { endpoints?: unknown } | undefined)?.endpoints);
  // Once the run queries what the bar shows, the change has landed.
  if (pendingEndpoints !== null && sameEndpoints(pendingEndpoints, runEndpoints)) {
    setPendingEndpoints(null);
  }
  const liveEndpoints = pendingEndpoints ?? runEndpoints;
  const addEndpoint = (name: string) => {
    if (!liveEndpoints.includes(name) && liveEndpoints.length < MAX_ENDPOINTS) {
      setPendingEndpoints([...liveEndpoints, name]);
    }
  };
  const liveBar = run ? (
    <div className="flex flex-wrap items-start gap-1">
      <DataBar project={run.project} selected={liveEndpoints} onChange={setPendingEndpoints} />
      {record.data?.kind === "conversation" ? (
        <CapabilitiesControl
          project={run.project}
          endpoints={liveEndpoints}
          value={capabilities}
          onChange={chooseCapabilities}
        />
      ) : null}
    </div>
  ) : null;

  if (!open) {
    const openPanel = () => {
      focusBox.current = true;
      setOpen(true);
    };
    // The dot is for the eye; the name says it too, so a screen reader hears it (T-6230).
    const bubbleName = t(isBusy ? "assistant.openBusy" : "assistant.open");
    const busyDot = isBusy ? (
      <span
        data-testid="assistant-busy"
        className="absolute right-1 top-1 size-3 rounded-full bg-danger ring-2 ring-surface"
      />
    ) : null;
    // On a phone the button sits in the header with the other menus (T-4992): floating, it lay
    // over the last column of every list, the row's "..." menu included, wherever the page was
    // scrolled to.
    if (phone && launcherSlot) {
      return createPortal(
        <Button
          variant="ghost"
          size="sm"
          className="relative"
          aria-expanded={false}
          aria-label={bubbleName}
          title={bubbleName}
          data-assistant-bubble=""
          onClick={openPanel}
          icon={<Icon name="chat" className="size-5" />}
        >
          {busyDot}
        </Button>,
        launcherSlot,
      );
    }
    return (
      // Hand-made on purpose: a 56 px round floating button, which the shared Button's fixed
      // heights would fight class by class. No `aria-controls`: the panel it opens is not in the
      // page while it is closed, and a relationship to nothing leads nowhere.
      <button
        type="button"
        aria-expanded={false}
        aria-label={bubbleName}
        title={bubbleName}
        data-assistant-bubble=""
        onClick={openPanel}
        className="focus-ring fixed bottom-4 right-4 z-40 flex size-14 items-center justify-center rounded-full bg-primary text-primary-fg shadow-3 hover:bg-primary-hover"
      >
        <Icon name="chat" className="size-6" />
        {busyDot}
      </button>
    );
  }

  const conversation = table ? (
        <Card padding="sm" id="run-chat" data-testid="assistant-table" className="flex min-h-48 w-full flex-1 flex-col gap-3 overflow-y-auto">
          <Button size="sm" onClick={() => setTable(null)} className="self-start">
            {t("assistant.backToChat")}
          </Button>
          <Suspense fallback={<p className="text-caption text-fg-muted">{t("app.loading")}</p>}>
            <CompleteFlow key={`${activeProject}/${table.name}/${table.lastModified}`} project={activeProject} file={table} inDock />
          </Suspense>
        </Card>
      ) : building ? (
        <Card padding="sm" id="run-chat" data-testid="assistant-build" className="flex min-h-48 w-full flex-1 flex-col gap-3 overflow-y-auto">
          <Button
            size="sm"
            onClick={() => {
              setBuilding(false);
            }}
            className="self-start"
          >
            {t("assistant.backToChat")}
          </Button>
          <Suspense fallback={<p className="text-caption text-fg-muted">{t("app.loading")}</p>}>
            <AppGenerator
              project={activeProject}
              onStarted={(runId) => {
                setBuilding(false);
                rememberRun({ project: activeProject, runId });
              }}
            />
          </Suspense>
        </Card>
      ) : !run ? (
        <>
          <Card padding="sm" id="run-chat" data-testid="assistant-empty" className="flex min-h-48 w-full flex-1 flex-col gap-4 overflow-y-auto">
            <p className="text-body text-fg-muted">{t("assistant.empty.lead")}</p>
            {mayAsk ? null : (
              <Alert tone="info" data-testid="assistant-read-only">
                {t("assistant.readOnly")}
              </Alert>
            )}
            <Paths
              project={activeProject}
              disabled={isStarting}
              capabilities={capabilities}
              onPick={(path) => {
                void startConversation("", path);
              }}
            />
          </Card>

          <form
            className="flex flex-col gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              const text = composerMessage.trim();
              if (text && !isStarting && mayAsk) {
                void startConversation(text);
              }
            }}
          >
            <div className="flex flex-wrap items-start gap-1">
              <DataBar
                project={activeProject}
                selected={chosenEndpoints}
                onChange={chooseEndpoints}
                opens="down"
              />
              <CapabilitiesControl
                project={activeProject}
                endpoints={chosenEndpoints}
                value={capabilities}
                onChange={chooseCapabilities}
              />
            </div>
            {/* The failure is the composer's own error: tied to the box the person is still in,
                which goes invalid, and announced (UI-44, T-1749). What was typed stays. */}
            <Field
              id="assistant-empty-composer"
              label={t("assistant.empty.composer")}
              errors={startError ? [`${t("assistant.empty.failed")} ${startError}`] : undefined}
            >
              <Textarea
                id="assistant-empty-composer"
                rows={3}
                value={composerMessage}
                onChange={(event) => {
                  setComposerMessage(event.target.value);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    const text = composerMessage.trim();
                    if (text && !isStarting && mayAsk) {
                      void startConversation(text);
                    }
                  }
                }}
                disabled={!mayAsk}
                placeholder={mayAsk ? undefined : t("assistant.readOnlyComposer")}
                title={mayAsk ? undefined : t("assistant.readOnly")}
                className="resize-none"
              />
            </Field>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-1">
                {attach}
              </div>
              <Button
                type="submit"
                variant="primary"
                size="sm"
                disabled={composerMessage.trim() === "" || !mayAsk}
                disabledReason={mayAsk ? t("agentRun.conversation.writeFirst") : t("assistant.readOnly")}
                loading={isStarting}
              >
                {t("assistant.empty.send")}
              </Button>
            </div>
          </form>
        </>
      ) : (
        <>
          {picked !== null && run ? (
            <div data-testid="assistant-picked" className="flex items-center justify-between gap-2 rounded-md border border-border px-2 py-1 text-caption">
              <span className="min-w-0 truncate font-mono">{t("assistant.picked", { src: picked })}</span>
              <Button
                size="xs"
                variant="ghost"
                onClick={() => {
                  pickElement(run.runId, null);
                }}
              >
                {t("assistant.pickedClear")}
              </Button>
            </div>
          ) : null}
          <Card flush id="run-chat" className="min-h-48 w-full flex-1 [&>section]:h-full [&>section]:min-h-0">
            <ConversationPanel
              project={run.project}
              events={events}
              streaming={streaming}
              answering={answer.isPending}
              sending={send.isPending}
              live={!over}
              building={Boolean(record.data?.appName)}
              // The run Build an app's card started is the one the dock follows next (T-2721).
              onBuildStarted={(runId) => {
                rememberRun({ project: activeProject, runId });
              }}
              onAnswer={(questionId, answers) => {
                answer.mutate({ questionId, answers });
              }}
              // `mutateAsync`, so the panel knows whether the message left: it empties the box on
              // success and keeps every word of it, with the reason, when the send failed (T-1761).
              // Each message of a conversation says the page it was sent from: "and this one?" is
              // about where the person is now (T-2763). An application run takes no page.
              onSend={(text) =>
                send.mutateAsync({
                  text,
                  // One message is scoped to what the person pointed at; the next one is not (SDK-46).
                  ...(picked !== null ? { scope: picked } : {}),
                  pageContext: record.data?.kind === "conversation" ? pageContext(activeProject) : undefined,
                  // What the person switched on or off travels with each message (AG-92).
                  access: record.data?.kind === "conversation" ? accessFor(capabilities, liveEndpoints) : undefined,
                  ...(pendingEndpoints !== null && !sameEndpoints(pendingEndpoints, runEndpoints)
                    ? { endpointNames: pendingEndpoints }
                    : {}),
                }).then((sent) => {
                  if (picked !== null && run) {
                    pickElement(run.runId, null);
                  }
                  return sent;
                })
              }
              onCancel={() => cancel.mutate()}
              onRetry={retry}
              onNewConversation={newConversation}
              attach={attach}
              above={full && wide ? undefined : liveBar}
              onUseEndpoint={addEndpoint}
              // A link in an answer opens its page in place; from full screen, beside it, so the
              // person sees the page and keeps the conversation (T-2773).
              onOpenLink={(href) => {
                if (full) {
                  setLayout("side");
                }
                void navigate({ href });
              }}
              usedEndpoints={liveEndpoints}
            />
          </Card>
        </>
      );

  return (
    <aside
      aria-label={t("agentRun.conversation.title")}
      data-layout={layout}
      className={
        full
          ? "fixed inset-x-0 bottom-0 top-14 z-40 flex flex-col gap-2 bg-surface p-3"
          : layout === "float"
            ? clsx(
                // Computed from the viewport: 40 rem tall or the screen less the gaps, and the
                // screen's width less the two 1 rem gaps, capped below.
                "fixed bottom-4 right-4 z-40 flex h-[min(40rem,calc(100vh-5rem))] w-[calc(100vw-2rem)] flex-col gap-2 rounded-lg border border-border bg-surface p-3 shadow-3",
                // The app builder is a form: it gets the room a form needs.
                building ? "max-w-176" : "max-w-104",
                // On a phone a floating panel left the page showing round it (T-4204): below sm it
                // covers the screen under the header, as the side layout does there.
                "max-sm:inset-x-0 max-sm:bottom-0 max-sm:right-auto max-sm:top-14 max-sm:h-auto max-sm:w-full max-sm:max-w-none max-sm:rounded-none max-sm:border-0 max-sm:shadow-none",
              )
            : clsx(
                // Below md there is no room for a column beside the page: the panel covers the
                // screen under the header, as on full screen. In the row it squeezed the page to
                // no width, and the page's positioned controls painted through it (T-2854).
                "fixed inset-x-0 bottom-0 top-14 z-40 flex flex-col gap-2 bg-surface p-3",
                // Computed: the viewport's height less the 14 header it sticks under. Beside the
                // 16 rem sidebar the page keeps at least 40 rem: the chat docks from xl, the
                // builder (40 rem itself) from 2xl; at 768 px the docked builder left the page
                // one letter wide (T-3145).
                building
                  ? "2xl:sticky 2xl:inset-auto 2xl:top-14 2xl:z-auto 2xl:h-below-header 2xl:shrink-0 2xl:border-l 2xl:border-border 2xl:w-160"
                  : "xl:sticky xl:inset-auto xl:top-14 xl:z-auto xl:h-below-header xl:shrink-0 xl:border-l xl:border-border xl:w-96",
              )
      }
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-baseline gap-2">
          {/* No `shrink-0`: in de, cs and sk the title is long enough to push the app name and the
              buttons out of the panel; it wraps instead (Button.tsx says why). */}
          <h2 className="text-body font-semibold">{t("assistant.title")}</h2>
          {run && buildingApp ? (
            <span data-testid="assistant-app" title={record.data?.appName} className="truncate text-caption text-fg-muted">
              {buildingApp}
            </span>
          ) : null}
        </div>
        <div className="flex items-center gap-1">
          {run ? (
            <Button
              variant="ghost"
              size="sm"
              aria-label={t("assistant.newConversation")}
              title={t("assistant.newConversation")}
              onClick={newConversation}
              className={iconButton}
              icon={<Icon name="plus" className="size-4" />}
            />
          ) : null}
          {run && !over ? (
            <Button
              variant="ghost"
              size="sm"
              aria-label={t("assistant.cancel")}
              title={t("assistant.cancel")}
              loading={cancel.isPending}
              onClick={() => {
                cancel.mutate();
              }}
              className={iconButton}
              icon={<Icon name="stop" className="size-4" />}
            />
          ) : null}
          {LAYOUTS.map(({ value, icon, label }) => (
            <Button
              key={value}
              variant="ghost"
              size="sm"
              aria-pressed={layout === value}
              aria-label={t(label)}
              title={t(label)}
              onClick={() => {
                setLayout(value);
              }}
              className={iconButton}
              icon={<Icon name={icon} className="size-4" />}
            />
          ))}
          <Button
            variant="ghost"
            size="sm"
            aria-expanded={open}
            aria-controls="run-chat"
            aria-label={t("assistant.hide")}
            title={t("assistant.hide")}
            onClick={() => {
              setOpen(false);
            }}
            className={iconButton}
            icon={<Icon name="minimize" className="size-4" />}
          />
          <Button
            variant="ghost"
            size="sm"
            aria-label={t("assistant.close")}
            title={t("assistant.close")}
            onClick={() => {
              rememberRun(null);
              setOpen(false);
            }}
            className={iconButton}
            icon={<Icon name="close" className="size-4" />}
          />
        </div>
      </div>

      {navigated !== null && isPortalRoute(navigated) ? (
        <Alert
          tone="info"
          actions={
            <Button
              variant="ghost"
              size="xs"
              onClick={() => {
                dismissNotice();
              }}
            >
              {t("assistant.dismiss")}
            </Button>
          }
        >
          {t("assistant.navigated", { page: pageOf(navigated, t) })}
        </Alert>
      ) : null}

      {opened.length > 1 ? (
        <nav
          aria-label={t("assistant.trail")}
          data-testid="assistant-trail"
          className="flex w-full flex-wrap items-center gap-2 px-1 text-caption text-fg-muted"
        >
          {opened.map((route) => (
            <Button
              key={route}
              variant="ghost"
              size="xs"
              onClick={() => {
                void navigate({ href: route });
              }}
              className="text-fg-muted"
            >
              {pageOf(route, t)}
            </Button>
          ))}
        </nav>
      ) : null}

      {/* Full screen (T-2773, UI-84): the conversations on the left, the conversation in one
          column of a readable width, its data and capabilities on the right. From 1024 px; below
          it the conversation is the screen. At 2560 px the sides widen, the column does not.
          The same tree in every layout, `contents` where it is not full screen, so switching
          keeps the conversation and every word typed into it. */}
      <div data-testid={full ? "assistant-full" : undefined} className={full ? "flex min-h-0 flex-1 gap-4" : "contents"}>
          {full && wide ? (
            <Side
              id="assistant-conversations"
              open={listOpen}
              onToggle={setListOpen}
              show={t("assistant.full.showList")}
              hide={t("assistant.full.hideList")}
              edge="left"
              className="w-64 2xl:w-80"
            >
              <ConversationList
                project={activeProject}
                current={run?.runId ?? null}
                onOpen={(runId) => {
                  setBuilding(false);
                  rememberRun({ project: activeProject, runId });
                }}
              />
            </Side>
          ) : null}
          <div
            data-testid={full ? "assistant-column" : undefined}
            className={full ? "mx-auto flex min-h-0 w-full max-w-4xl min-w-0 flex-1 flex-col gap-2" : "contents"}
          >
            {conversation}
          </div>
          {full && wide && run && !building ? (
            <Side
              id="assistant-data"
              open={panelOpen}
              onToggle={setPanelOpen}
              show={t("assistant.full.showData")}
              hide={t("assistant.full.hideData")}
              edge="right"
              className="w-80 2xl:w-112"
            >
              <section aria-label={t("assistant.full.data")} className="flex flex-col gap-2">
                <h3 className="text-caption font-semibold text-fg-muted">{t("assistant.full.data")}</h3>
                {liveBar}
              </section>
            </Side>
          ) : null}
      </div>

    </aside>
  );
}

/** Wide enough for the conversations and the data beside the full-screen conversation. */
const WIDE = "(min-width: 64rem)";

function onWideChange(changed: () => void): () => void {
  const query = typeof window.matchMedia === "function" ? window.matchMedia(WIDE) : null;
  query?.addEventListener("change", changed);
  return () => query?.removeEventListener("change", changed);
}

function isWide(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(WIDE).matches;
}

/**
 * A side of the full-screen assistant that folds away to a button, so the conversation can take
 * the width when the person wants it (T-2773).
 */
function Side({
  id,
  open,
  onToggle,
  show,
  hide,
  edge,
  className,
  children,
}: {
  id: string;
  open: boolean;
  onToggle: (open: boolean) => void;
  show: string;
  hide: string;
  edge: "left" | "right";
  className: string;
  children: ReactNode;
}): JSX.Element {
  const toggle = (
    <Button
      variant="ghost"
      size="sm"
      aria-expanded={open}
      aria-controls={open ? id : undefined}
      aria-label={open ? hide : show}
      title={open ? hide : show}
      onClick={() => {
        onToggle(!open);
      }}
      className="w-8 shrink-0 self-start px-0 text-fg-muted hover:text-fg"
      icon={<Icon name={open === (edge === "left") ? "chevronLeft" : "chevronRight"} className="size-4" />}
    />
  );
  return (
    <div className={clsx("flex min-h-0 shrink-0 gap-1", edge === "right" && "flex-row-reverse", open && className)}>
      {toggle}
      {open ? (
        <div id={id} className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
          {children}
        </div>
      ) : null}
    </div>
  );
}

/** Beside the page, floating over it, or the whole screen; the bubble is the fourth, closed state. */
type Layout = "side" | "float" | "full";

const LAYOUT_KEY = "jc.assistant.layout";

const LAYOUTS: { value: Layout; icon: IconName; label: string }[] = [
  { value: "side", icon: "sidebar", label: "assistant.sideView" },
  { value: "float", icon: "float", label: "assistant.floatView" },
  { value: "full", icon: "expand", label: "assistant.fullScreen" },
];

function storedLayout(): Layout {
  try {
    const stored = sessionStorage.getItem(LAYOUT_KEY);
    return stored === "float" || stored === "full" ? stored : "side";
  } catch {
    return "side";
  }
}

function routeOf(event: RunEvent): string | null {
  const route = event.payload.route;
  return isPortalRoute(route) ? route : null;
}

function prefillOf(event: RunEvent): Record<string, unknown> {
  const prefill = event.payload.prefill;
  return typeof prefill === "object" && prefill !== null && !Array.isArray(prefill)
    ? (prefill as Record<string, unknown>)
    : {};
}
