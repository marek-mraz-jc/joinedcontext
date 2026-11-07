import { useEffect, useEffectEvent, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { digestOf, getDraft, putDraft, subscribeDrafts } from "../../api/drafts";
import type { Draft, DraftEvent, Verdict } from "../../api/drafts";
import { extractName, manifestOf } from "./resourceForm";
import type { ManifestSource } from "./types";

export interface ResourceDraftOptions<T> {
  open: boolean;
  project?: string;
  draftKind?: string;
  draftName?: string;
  formData?: T;
  source?: ManifestSource<T>;
  /** The digest of the manifest a page builds around the form (a hidden attribute lives outside it). */
  currentDigest: string;
  /** The page's own verdict, when it holds one; the hook's otherwise. */
  verdict?: Verdict | null;
  onVerdictChange?: (verdict: Verdict | null) => void;
  onChange?: (data: T | undefined) => void;
}

export interface ResourceDraft<T> {
  /** The name the draft is kept under: the page's `draftName`, else the form's own name. */
  activeName: string | undefined;
  currentDraft: Draft | null;
  verdict: Verdict | null;
  updateVerdict: (verdict: Verdict | null) => void;
  conflict: string | null;
  setConflict: (conflict: string | null) => void;
  /** Why the server would not save the draft (MF-24, T-2626), in its own words. */
  saveRefused: string | null;
  /**
   * The draft a proposal names, holding what the form shows: a proposal takes the draft's own
   * manifest, so the save the debounce has not sent yet goes first (T-0769, AG-61).
   */
  draftRefFor: (form: T) => Promise<{ kind: string; name: string } | undefined>;
  /** Forgets the draft on close, so the next open loads it again. */
  reset: () => void;
}

/** How long the form rests after a change before its draft is written. */
const DRAFT_WRITE_MS = 600;
/** A draft event this soon after a keystroke is the person's own write coming back. */
const OWN_ECHO_MS = 1500;

/** The server's own sentence for a refused save, else what the request failed with. */
function refusalOf(err: unknown, generic: string): string {
  const detail = (err as { detail?: unknown } | null)?.detail;
  if (typeof detail === "string" && detail) return detail;
  return err instanceof Error ? err.message : generic;
}

/** The refs the draft's hooks share: what the store last answered and when the person typed. */
interface DraftRefs {
  lastVersion: number | undefined;
  lastTyped: number;
  /** The digest of the manifest the draft last held: an unchanged form writes nothing. */
  syncedDigest: string | undefined;
}

/** The verdict a form shows: the page's own when it holds one, else the hook's. */
interface HeldVerdict {
  verdict: Verdict | null;
  updateVerdict: (verdict: Verdict | null) => void;
  adoptVerdict: (incoming: Verdict | null | undefined) => void;
}

/** What the draft's hooks share: its address, its refs, its setters and its verdict. */
interface DraftCore<T> {
  project?: string;
  draftKind?: string;
  draftName?: string;
  activeName?: string;
  source?: ManifestSource<T>;
  onChange?: (data: T | undefined) => void;
  loadedName: string | undefined;
  setLoadedName: (name: string | undefined) => void;
  setCurrentDraft: (draft: Draft | null) => void;
  setConflict: (conflict: string | null) => void;
  setSaveRefused: (refused: string | null) => void;
  draftRef: { current: DraftRefs };
  held: HeldVerdict;
}

function useHeldVerdict(
  pageVerdict: Verdict | null | undefined,
  onVerdictChange?: (verdict: Verdict | null) => void,
): HeldVerdict {
  const [ownVerdict, setOwnVerdict] = useState<Verdict | null>(null);
  /** The verdict on screen, for the answers below that arrive after a newer one (T-2624). */
  const heldRef = useRef<Verdict | null>(null);
  const updateVerdict = (next: Verdict | null) => {
    heldRef.current = next;
    setOwnVerdict(next);
    onVerdictChange?.(next);
  };
  /**
   * The verdict a save or a reload of the draft carries. The store keeps a draft's verdict
   * across a save, so a save that crosses a check in flight answers with the verdict from before
   * that check (none, or the one another hand ran on its own manifest). Taken as it came, it
   * replaced the fresh one and Propose stayed refused (T-2624). The newer check wins; the digest
   * still decides whether it is fresh for what the form holds (PF-57).
   */
  const adoptVerdict = (incoming: Verdict | null | undefined) => {
    if (incoming === undefined) {
      return;
    }
    const held = heldRef.current;
    if (held !== null && (incoming === null || Date.parse(incoming.checkedAt) < Date.parse(held.checkedAt))) {
      return;
    }
    updateVerdict(incoming);
  };
  return { verdict: pageVerdict !== undefined ? pageVerdict : ownVerdict, updateVerdict, adoptVerdict };
}

/**
 * A draft opened is not the person's edit: what the form would write from it counts as synced,
 * so a draft holding a field the form has no control for is not rewritten without it on open.
 * Measured by the stored manifest instead, that write-back waited only on a re-render clearing
 * the debounce first (T-2754).
 */
function showManifest<T>(core: DraftCore<T>, manifest: unknown) {
  const { source } = core;
  const loaded = source ? source.fromManifest(manifest) : (manifest as T);
  core.draftRef.current.syncedDigest = digestOf(source ? source.toManifest(loaded) : manifest);
  core.onChange?.(loaded);
}

function takeDraft<T>(core: DraftCore<T>, draft: Draft) {
  core.setCurrentDraft(draft);
  core.draftRef.current.lastVersion = draft.version;
}

/** A draft another hand wrote, read back: its version, its verdict and its manifest. */
function takeReloaded<T>(core: DraftCore<T>, reloaded: Draft | null) {
  if (!reloaded) return;
  takeDraft(core, reloaded);
  core.held.adoptVerdict(reloaded.verdict);
  if (reloaded.manifest) showManifest(core, reloaded.manifest);
}

/** The store's answer to this form's own write of `digest`. */
function takeSaved<T>(core: DraftCore<T>, saved: Draft, digest: string) {
  core.draftRef.current.syncedDigest = digest;
  takeDraft(core, saved);
  core.held.adoptVerdict(saved.verdict);
  core.setSaveRefused(null);
}

/** The address a draft is read and written at, once the form names one. */
function addressOf<T>(core: DraftCore<T>, open: boolean): { project: string; kind: string; name: string } | null {
  const { project, draftKind, activeName } = core;
  return open && project && draftKind && activeName ? { project, kind: draftKind, name: activeName } : null;
}

// Load existing draft on open if draftName is set
function useDraftLoad<T>(core: DraftCore<T>, open: boolean) {
  const { project, draftKind, draftName } = core;
  const { t } = useTranslation();
  const onFailed = useEffectEvent((err: unknown) => core.setSaveRefused(refusalOf(err, t("app.error.generic"))));
  const onLoaded = useEffectEvent((d: Draft | null) => {
    core.setLoadedName(draftName);
    if (!d) return;
    takeDraft(core, d);
    if (d.verdict !== undefined) {
      core.held.updateVerdict(d.verdict ?? null);
    }
    if (d.manifest) showManifest(core, d.manifest);
  });
  useEffect(() => {
    if (!open || !draftKind || !project || !draftName) {
      return;
    }
    let active = true;
    void getDraft(project, draftKind, draftName).then(
      (d) => {
        if (active) onLoaded(d);
      },
      // Unread is not empty: the name stays unloaded, so nothing is written over the draft
      // another window may hold, and the person is told why (F-UI1-013).
      (err: unknown) => {
        if (active) onFailed(err);
      },
    );
    return () => {
      active = false;
    };
  }, [open, draftKind, project, draftName]);
}

// Debounced write of the draft on a form change. `currentDigest` stands for the manifest a page
// builds around the form, which `formData` alone does not move.
function useDraftWrite<T>(core: DraftCore<T>, open: boolean, formData: T | undefined, currentDigest: string) {
  const { t } = useTranslation();
  const { draftName, loadedName, draftRef } = core;
  const write = (at: { project: string; kind: string; name: string }, manifest: unknown, digest: string) => {
    // A proposal may have written this manifest already (see `draftRefFor`).
    if (digest === draftRef.current.syncedDigest) {
      return;
    }
    void putDraft(at.project, at.kind, at.name, manifest, draftRef.current.lastVersion)
      .then((d) => {
        takeSaved(core, d, digest);
        core.setConflict(null);
      })
      .catch((err: unknown) => {
        if ((err as { status?: number } | null)?.status !== 409) {
          core.setSaveRefused(refusalOf(err, t("app.error.generic")));
          return;
        }
        core.setConflict(t("drafts.conflict"));
        // A read-back that fails leaves the conflict said; the next save tries again.
        void getDraft(at.project, at.kind, at.name).then((d) => takeReloaded(core, d), () => undefined);
      });
  };
  const scheduleWrite = useEffectEvent((form: T | undefined): (() => void) | undefined => {
    const at = addressOf(core, open);
    // A named draft is loaded before anything is written back: the form's first, empty
    // data must not overwrite what another window typed.
    if (!at || !form || (draftName && loadedName !== draftName)) {
      return undefined;
    }
    const manifest = manifestOf(form, core.source);
    const digest = digestOf(manifest);
    if (digest === draftRef.current.syncedDigest) {
      return undefined;
    }
    draftRef.current.lastTyped = Date.now();
    const timer = setTimeout(() => write(at, manifest, digest), DRAFT_WRITE_MS);
    return () => clearTimeout(timer);
  });
  const { project, draftKind, activeName } = core;
  useEffect(
    () => scheduleWrite(formData),
    [formData, currentDigest, open, draftKind, project, activeName, draftName, loadedName],
  );
}

// Subscribe to project draft events
function useDraftEvents<T>(core: DraftCore<T>, open: boolean, hasDraft: boolean) {
  const { draftRef } = core;
  const onDraftEvent = useEffectEvent((event: DraftEvent) => {
    const at = addressOf(core, true);
    if (!at || event.kind !== at.kind || event.name !== at.name) {
      return;
    }
    if (event.event === "drop") {
      core.setCurrentDraft(null);
      core.held.updateVerdict(null);
      return;
    }
    if (event.event === "verdict") {
      void getDraft(at.project, at.kind, at.name).then(
        (d) => {
          if (d) {
            core.setCurrentDraft(d);
            core.held.adoptVerdict(d.verdict ?? null);
          }
        },
        // A refresh another window prompted: one that fails keeps what the form shows.
        () => undefined,
      );
      return;
    }
    if (event.version > (draftRef.current.lastVersion ?? 0) && Date.now() - draftRef.current.lastTyped >= OWN_ECHO_MS) {
      void getDraft(at.project, at.kind, at.name).then((d) => takeReloaded(core, d), () => undefined);
    }
  });
  const { project, draftKind, activeName } = core;
  useEffect(() => {
    if (!open || !draftKind || !project || !hasDraft) {
      return;
    }
    return subscribeDrafts(project, (event) => onDraftEvent(event));
  }, [open, draftKind, project, activeName, hasDraft]);
}

/**
 * The draft a proposal names, holding what the form shows: the save the debounce has not sent
 * yet goes first (T-0769, AG-61).
 */
async function draftRefOf<T>(
  core: DraftCore<T>,
  form: T,
  generic: string,
): Promise<{ kind: string; name: string } | undefined> {
  const { project, draftKind, draftName, draftRef } = core;
  const active = draftName || extractName(form);
  if (!draftKind || !active) {
    return undefined;
  }
  const manifest = manifestOf(form, core.source);
  const digest = digestOf(manifest);
  const loaded = !draftName || core.loadedName === draftName;
  if (project && loaded && digest !== draftRef.current.syncedDigest) {
    try {
      takeSaved(core, await putDraft(project, draftKind, active, manifest, draftRef.current.lastVersion), digest);
    } catch (err) {
      // Another window changed the draft: the person sees it before anything is proposed. Any
      // other failure leaves the proposal to the server's check, which says what to do.
      if ((err as { status?: number }).status === 409) {
        throw err;
      }
      core.setSaveRefused(refusalOf(err, generic));
    }
  }
  return { kind: draftKind, name: active };
}

/**
 * The draft a form edits (PF-57, MF-24): loaded on open, written 600 ms after a change that moved
 * its digest, reloaded when another window writes it, and the verdict it carries.
 */
export function useResourceDraft<T>(options: ResourceDraftOptions<T>): ResourceDraft<T> {
  const { open, draftName, formData } = options;
  const { t } = useTranslation();
  const [currentDraft, setCurrentDraft] = useState<Draft | null>(null);
  // The draft name whose load has answered, with or without a draft: the write below waits
  // for it, and a name that has no draft yet must not wait forever (T-0631).
  const [loadedName, setLoadedName] = useState<string | undefined>(undefined);
  const [conflict, setConflict] = useState<string | null>(null);
  const [saveRefused, setSaveRefused] = useState<string | null>(null);
  const draftRef = useRef<DraftRefs>({ lastVersion: undefined, lastTyped: 0, syncedDigest: undefined });
  const held = useHeldVerdict(options.verdict, options.onVerdictChange);
  const activeName = draftName || extractName(formData);
  const core: DraftCore<T> = {
    ...options,
    activeName,
    loadedName,
    setLoadedName,
    setCurrentDraft,
    setConflict,
    setSaveRefused,
    draftRef,
    held,
  };

  useDraftLoad(core, open);
  useDraftWrite(core, open, formData, options.currentDigest);
  useDraftEvents(core, open, currentDraft !== null);

  function reset() {
    setConflict(null);
    setCurrentDraft(null);
    setLoadedName(undefined);
    draftRef.current.lastVersion = undefined;
  }

  return {
    activeName,
    currentDraft,
    verdict: held.verdict,
    updateVerdict: held.updateVerdict,
    conflict,
    setConflict,
    saveRefused,
    draftRefFor: (form) => draftRefOf(core, form, t("app.error.generic")),
    reset,
  };
}
