import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { download } from "@joinedcontext/sdk";
import { api, ApiError, unwrap } from "../../api/client";
import { useIdentity } from "../../auth/AuthProvider";
import { Alert, Button, ConfirmDialog, Field, Input, PermissionGuard } from "../../components/ui";

/** The server's limits (src/erasure.rs MAX_SUBJECT, MAX_REASON). */
const MAX_SUBJECT = 256;
const MAX_REASON = 500;

interface Preview {
  entities: number;
  groups: { class: string; entities: string[]; attrs?: string[] | null }[];
}

interface Erasure {
  id: string;
  state: "proposed" | "expired" | "declined" | "open" | "done" | "failed";
  requestedAt: string;
  completedAt?: string | null;
  proposedBy: string;
  expiresAt?: string;
  entities: number;
  stores: Record<string, { state: string; count: number; versions?: number; error?: string }>;
}

/** What the server would refuse about the subject (erasure::subject_refusal); `null` when fine. */
function subjectProblem(subject: string): "required" | "tooLong" | "characters" | null {
  const trimmed = subject.trim();
  if (trimmed === "") return "required";
  if ([...trimmed].length > MAX_SUBJECT) return "tooLong";
  // The server refuses quotes, backslashes and control characters (erasure::subject_refusal).
  return [...trimmed].some((c) => c === '"' || c === "\\" || c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127)
    ? "characters"
    : null;
}

function reasonProblem(reason: string): "required" | "tooLong" | null {
  const trimmed = reason.trim();
  if (trimmed === "") return "required";
  return [...trimmed].length > MAX_REASON ? "tooLong" : null;
}

/** A time as the person reads it, in their language. */
function when(at: string | undefined, language: string): string {
  if (at === undefined) return "";
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(language, { dateStyle: "medium", timeStyle: "short" });
}

/**
 * A data subject's rights in a space (DM-124, DM-126, DM-127, DM-142, API/01 §46): an owner of the
 * space exports what the space holds about one person, or proposes erasing it after a preview of
 * what goes; another owner approves the proposal with the person's identifier typed again, and
 * only then is anything erased. The person's value travels in the request body only; every call is
 * audited with its reason. Others see the section and why its buttons are off, and the server
 * refuses them again.
 */
export function SpacePersonalData({ project, space }: { project: string; space: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [subject, setSubject] = useState("");
  const [reason, setReason] = useState("");
  const [touched, setTouched] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [exported, setExported] = useState<number | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [erasureId, setErasureId] = useState<string | null>(null);

  const path = { project, space };
  const body = () => ({ subject: subject.trim(), reason: reason.trim() });
  const said = (error: unknown) =>
    error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error);
  const subjectError = touched ? subjectProblem(subject) : null;
  const reasonError = touched ? reasonProblem(reason) : null;

  /** Runs `then` when both fields hold what the server accepts; marks them otherwise. */
  const valid = (then: () => void) => {
    setTouched(true);
    setProblem(null);
    setExported(null);
    if (subjectProblem(subject) === null && reasonProblem(reason) === null) then();
  };

  const exportRecords = useMutation({
    mutationFn: async () =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/spaces/{space}/subjects/export", {
          params: { path },
          body: body(),
        }),
      )) as { entities?: unknown[] },
    onSuccess: (answer) => {
      // The file exists only in this browser: the Portal keeps no export (DM-126).
      download(
        new Blob([JSON.stringify(answer, null, 2)], { type: "application/json" }),
        `personal-data-${space}-${new Date().toISOString().slice(0, 10)}.json`,
      );
      setExported(answer.entities?.length ?? 0);
    },
    onError: (error) => setProblem(said(error)),
  });

  const dryRun = useMutation({
    mutationFn: async () =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/spaces/{space}/erasures", {
          params: { path, query: { dryRun: "All" } },
          body: body(),
        }),
      )) as Preview,
    onSuccess: setPreview,
    onError: (error) => setProblem(said(error)),
  });

  const erase = useMutation({
    mutationFn: async () =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/spaces/{space}/erasures", {
          params: { path },
          body: body(),
        }),
      )) as Erasure,
    onSuccess: (started) => {
      setPreview(null);
      setErasureId(started.id);
      void proposals.refetch();
      // The value leaves the page once it is erased: nothing keeps it on screen.
      setSubject("");
      setTouched(false);
    },
    onError: (error) => {
      setPreview(null);
      setProblem(said(error));
    },
  });

  // An erasure stays open until every store reports done (API/01 §46): asked again while open.
  const erasure = useQuery({
    queryKey: ["projects", project, "spaces", space, "erasures", erasureId],
    enabled: erasureId !== null,
    retry: false,
    refetchInterval: (query) => (query.state.data?.state === "open" ? 5_000 : false),
    queryFn: async () =>
      (await unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/erasures/{id}", {
          params: { path: { project, space, id: erasureId ?? "" } },
        }),
      )) as Erasure,
  });
  const shown = erasure.data ?? (erase.data?.id === erasureId ? erase.data : undefined);

  const proposals = useQuery({
    queryKey: ["projects", project, "spaces", space, "erasures"],
    retry: false,
    queryFn: async () =>
      (await unwrap(await api.GET("/api/v1/projects/{project}/spaces/{space}/erasures", { params: { path } }))) as {
        items: Erasure[];
      },
  });

  // A 403 on the list is the role, not a fault: said as a quiet line, never a red alert.
  const ownersOnly = proposals.error instanceof ApiError && proposals.error.status === 403;

  return (
    <div className="space-y-4">
      <p className="text-body text-fg-muted">{t("spaces.personal.lead")}</p>
      <div className="grid gap-4 md:grid-cols-2">
        <Field
          id="personal-subject"
          label={t("spaces.personal.subject")}
          required
          help={t("spaces.personal.hint.subject")}
          errors={subjectError ? [t(`spaces.personal.subjectError.${subjectError}`, { max: MAX_SUBJECT })] : undefined}
        >
          <Input
            id="personal-subject"
            value={subject}
            autoComplete="off"
            maxLength={MAX_SUBJECT}
            onChange={(event) => setSubject(event.target.value)}
          />
        </Field>
        <Field
          id="personal-reason"
          label={t("spaces.personal.reason")}
          required
          help={t("spaces.personal.hint.reason")}
          errors={reasonError ? [t(`spaces.personal.reasonError.${reasonError}`, { max: MAX_REASON })] : undefined}
        >
          <Input
            id="personal-reason"
            value={reason}
            maxLength={MAX_REASON}
            onChange={(event) => setReason(event.target.value)}
          />
        </Field>
      </div>
      <span className="flex flex-wrap gap-2">
        <PermissionGuard project={project} kind="ContextSpace" verb="approve">
          <Button
            variant="secondary"
            loading={exportRecords.isPending}
            onClick={() => valid(() => exportRecords.mutate())}
          >
            {t("spaces.personal.export")}
          </Button>
        </PermissionGuard>
        <PermissionGuard project={project} kind="ContextSpace" verb="approve">
          <Button variant="danger" loading={dryRun.isPending} onClick={() => valid(() => dryRun.mutate())}>
            {t("spaces.personal.erase")}
          </Button>
        </PermissionGuard>
      </span>
      {problem !== null && <Alert tone="danger">{problem}</Alert>}
      {exported !== null && (
        <Alert tone="success">{t("spaces.personal.exported", { count: exported })}</Alert>
      )}
      {shown !== undefined && (
        <Alert tone={shown.state === "failed" ? "danger" : shown.state === "done" ? "success" : "info"}>
          <p>
            {t(`spaces.personal.state.${shown.state}`, {
              id: shown.id,
              count: shown.entities,
              expires: when(shown.expiresAt, i18n.language),
            })}
          </p>
          {["open", "done", "failed"].includes(shown.state) && (
          <ul className="mt-2 list-disc pl-5">
            {Object.entries(shown.stores).map(([store, held]) => (
              <li key={store}>
                {t("spaces.personal.store", {
                  store: t(`spaces.personal.stores.${store}`, { defaultValue: store }),
                  state: t(`spaces.personal.storeState.${held.state}`, { defaultValue: held.state }),
                  count: held.count,
                })}
                {held.error ? ` ${held.error}` : ""}
              </li>
            ))}
          </ul>
          )}
        </Alert>
      )}
      <ErasureProposals
        project={project}
        space={space}
        items={(proposals.data?.items ?? []).filter((item) => item.state === "proposed")}
        failed={proposals.isError && !ownersOnly ? said(proposals.error) : null}
        ownersOnly={ownersOnly}
        onDecided={(id) => {
          setErasureId(id);
          void proposals.refetch();
        }}
      />
      {erasure.isError && <Alert tone="danger">{said(erasure.error)}</Alert>}
      <ConfirmDialog
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open) setPreview(null);
        }}
        title={t("spaces.personal.confirmTitle", { space })}
        description={t("spaces.personal.confirmLead")}
        confirmLabel={t("spaces.personal.confirm")}
        pending={erase.isPending}
        onConfirm={() => erase.mutate()}
      >
        {preview !== null && preview.entities === 0 ? (
          <p className="text-body">{t("spaces.personal.nothing")}</p>
        ) : (
          <ul className="list-disc pl-5 text-body">
            {(preview?.groups ?? []).map((group) => (
              <li key={group.class}>
                {group.attrs
                  ? t("spaces.personal.wouldErase.attrs", {
                      count: group.entities.length,
                      type: group.class,
                      attrs: group.attrs.join(", "),
                    })
                  : t("spaces.personal.wouldErase.entity", { count: group.entities.length, type: group.class })}
              </li>
            ))}
          </ul>
        )}
      </ConfirmDialog>
    </div>
  );
}

/**
 * The proposals waiting for a second owner (DM-142): each is approved with the person's
 * identifier typed again, or declined with a reason. The proposer sees why they cannot approve
 * their own; the server refuses it again (`403 self-approval`).
 */
function ErasureProposals({
  project,
  space,
  items,
  failed,
  ownersOnly = false,
  onDecided,
}: {
  project: string;
  space: string;
  items: Erasure[];
  failed: string | null;
  /** The caller is no owner of the space: the list is theirs to see only as one. */
  ownersOnly?: boolean;
  onDecided: (id: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const me = useIdentity();
  const [approving, setApproving] = useState<Erasure | null>(null);
  const [declining, setDeclining] = useState<Erasure | null>(null);
  const [typed, setTyped] = useState("");
  const [why, setWhy] = useState("");
  const [touched, setTouched] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const said = (error: unknown) =>
    error instanceof ApiError ? (error.problem?.detail ?? error.message) : String(error);
  const close = () => {
    setApproving(null);
    setDeclining(null);
    setTyped("");
    setWhy("");
    setTouched(false);
    setProblem(null);
  };
  const path = (id: string) => ({ project, space, id });

  const approve = useMutation({
    mutationFn: async (id: string) =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/spaces/{space}/erasures/{id}/approve", {
          params: { path: path(id) },
          body: { subject: typed.trim() },
        }),
      )) as Erasure,
    onSuccess: (approved) => {
      close();
      onDecided(approved.id);
    },
    onError: (error) => setProblem(said(error)),
  });
  const decline = useMutation({
    mutationFn: async (id: string) =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/spaces/{space}/erasures/{id}/decline", {
          params: { path: path(id) },
          body: { reason: why.trim() },
        }),
      )) as Erasure,
    onSuccess: (declined) => {
      close();
      onDecided(declined.id);
    },
    onError: (error) => setProblem(said(error)),
  });
  const typedError = touched ? subjectProblem(typed) : null;
  const whyError = touched ? reasonProblem(why) : null;

  return (
    <section aria-labelledby="erasure-proposals" className="space-y-2">
      <h3 id="erasure-proposals" className="text-body font-semibold">
        {t("spaces.personal.proposals.title")}
      </h3>
      {failed !== null && <Alert tone="danger">{failed}</Alert>}
      {ownersOnly ? <p className="text-body text-fg-muted">{t("spaces.personal.proposals.ownersOnly")}</p> : null}
      {failed === null && !ownersOnly && items.length === 0 && (
        <p className="text-body text-fg-muted">{t("spaces.personal.proposals.empty")}</p>
      )}
      <ul className="space-y-2">
        {items.map((item) => {
          const own = me !== null && item.proposedBy === `user:${me.username}`;
          return (
            <li key={item.id} className="flex flex-wrap items-center gap-2 rounded-md border border-border p-3">
              <span className="min-w-0 flex-1 text-body">
                {t("spaces.personal.proposals.item", {
                  id: item.id,
                  by: item.proposedBy.replace(/^user:/, ""),
                  at: when(item.requestedAt, i18n.language),
                  count: item.entities,
                  expires: when(item.expiresAt, i18n.language),
                })}
              </span>
              <PermissionGuard project={project} kind="ContextSpace" verb="approve">
                <Button
                  variant="danger"
                  size="sm"
                  disabled={own}
                  disabledReason={own ? t("spaces.personal.proposals.own") : undefined}
                  onClick={() => setApproving(item)}
                >
                  {t("spaces.personal.proposals.approve")}
                </Button>
              </PermissionGuard>
              <PermissionGuard project={project} kind="ContextSpace" verb="approve">
                <Button variant="secondary" size="sm" onClick={() => setDeclining(item)}>
                  {t("spaces.personal.proposals.decline")}
                </Button>
              </PermissionGuard>
            </li>
          );
        })}
      </ul>
      <ConfirmDialog
        open={approving !== null}
        onOpenChange={(open) => {
          if (!open) close();
        }}
        title={t("spaces.personal.approveTitle", { id: approving?.id ?? "" })}
        description={t("spaces.personal.approveLead")}
        confirmLabel={t("spaces.personal.approveConfirm")}
        pending={approve.isPending}
        onConfirm={() => {
          setTouched(true);
          if (approving !== null && subjectProblem(typed) === null) approve.mutate(approving.id);
        }}
      >
        <Field
          id="approve-subject"
          label={t("spaces.personal.subject")}
          required
          help={t("spaces.personal.hint.approveSubject")}
          errors={typedError ? [t(`spaces.personal.subjectError.${typedError}`, { max: MAX_SUBJECT })] : undefined}
        >
          <Input
            id="approve-subject"
            value={typed}
            autoComplete="off"
            maxLength={MAX_SUBJECT}
            onChange={(event) => setTyped(event.target.value)}
          />
        </Field>
        {problem !== null && <Alert tone="danger">{problem}</Alert>}
      </ConfirmDialog>
      <ConfirmDialog
        open={declining !== null}
        onOpenChange={(open) => {
          if (!open) close();
        }}
        title={t("spaces.personal.declineTitle", { id: declining?.id ?? "" })}
        description={t("spaces.personal.declineLead")}
        confirmLabel={t("spaces.personal.declineConfirm")}
        tone="primary"
        pending={decline.isPending}
        onConfirm={() => {
          setTouched(true);
          if (declining !== null && reasonProblem(why) === null) decline.mutate(declining.id);
        }}
      >
        <Field
          id="decline-reason"
          label={t("spaces.personal.reason")}
          required
          help={t("spaces.personal.hint.declineReason")}
          errors={whyError ? [t(`spaces.personal.reasonError.${whyError}`, { max: MAX_REASON })] : undefined}
        >
          <Input id="decline-reason" value={why} maxLength={MAX_REASON} onChange={(event) => setWhy(event.target.value)} />
        </Field>
        {problem !== null && <Alert tone="danger">{problem}</Alert>}
      </ConfirmDialog>
    </section>
  );
}
