import { reasonOf } from "../../api/client";
import { useState } from "react";
import type { JSX } from "react";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import type { TFunction } from "i18next";
import { usePermissions } from "../../api/permissions";
import { useDebounced } from "../../components/pickers/organizationModels";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { CodeBlock,
  Alert,
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Field,
  Input,
  PageFailed,
  PageHeader,
  Select,
  Switch,
} from "../../components/ui";
import { LocalRecordLink } from "../../components/RecordLink";
import { UsageSummary } from "./UsageSummary";
import { timeText } from "./knowledge";
import {
  aligned,
  capped,
  conversationKeys,
  costText,
  eraseConversation,
  evalCase,
  getConversation,
  listConversations,
  msText,
  sameQuestion,
  tokensText,
  totals,
  usdText,
  visible,
  visualChanges,
  wordDiff,
} from "./conversations";
import type { ConversationFilter, ConversationStep, ConversationTurn, DiffWord } from "./conversations";

/**
 * Asistent → Konverzácie (API/01 §34, T-4670): the project's stored assistant conversations,
 * redacted, for its administrator (`approve` on AssistantDeployment); every read is recorded.
 * One conversation opens as its turns, each with the steps that made the answer.
 */
export function ConversationsPage({
  project,
  open: opened,
  against,
}: {
  project: string;
  /** A conversation to open at once, and one to compare its last turn with (an eval run's link). */
  open?: string;
  against?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const { data: permissions, can } = usePermissions(project);
  const [filter, setFilter] = useState<ConversationFilter>({});
  const [open, setOpen] = useState<string | null>(opened ?? null);
  const allowed = can("AssistantDeployment", "approve");
  // Every list read is recorded: the search asks once the person stops typing, not per key.
  const [typed, setTyped] = useState("");
  const shown: ConversationFilter = { ...filter, q: useDebounced(typed) || undefined };
  const list = useQuery({
    queryKey: conversationKeys.list(project, shown),
    queryFn: () => listConversations(project, shown),
    enabled: allowed,
    placeholderData: keepPreviousData,
  });
  const set = (change: ConversationFilter) => {
    setFilter((now) => ({ ...now, ...change, cursor: undefined }));
  };

  if (permissions !== undefined && !allowed) {
    return (
      <section aria-label={t("conversations.title")} className="space-y-6">
        <PageHeader title={t("conversations.title")} description={t("conversations.intro")} />
        <Alert tone="info">{t("conversations.notAllowed")}</Alert>
      </section>
    );
  }
  const items = list.data?.items ?? [];
  return (
    <section aria-label={t("conversations.title")} className="space-y-6">
      <PageHeader title={t("conversations.title")} description={t("conversations.intro")} />
      <div className="flex flex-wrap items-end gap-3">
        <Field id="conv-q" label={t("conversations.filter.q")} className="w-64">
          <Input id="conv-q" type="search" value={typed} onChange={(e) => {
            setTyped(e.target.value);
            set({});
          }} />
        </Field>
        <Field id="conv-status" label={t("conversations.filter.status")} className="w-44">
          <Select
            id="conv-status"
            value={filter.status ?? ""}
            onChange={(e) => set({ status: (e.target.value || undefined) as ConversationFilter["status"] })}
          >
            <option value="">{t("conversations.filter.all")}</option>
            <option value="ok">{t("conversations.status.ok")}</option>
            <option value="error">{t("conversations.status.erred")}</option>
            <option value="timeout">{t("conversations.status.timeout")}</option>
          </Select>
        </Field>
        <Field id="conv-feedback" label={t("conversations.filter.feedback")} className="w-44">
          <Select
            id="conv-feedback"
            value={filter.feedback ?? ""}
            onChange={(e) => set({ feedback: (e.target.value || undefined) as ConversationFilter["feedback"] })}
          >
            <option value="">{t("conversations.filter.all")}</option>
            <option value="up">{t("conversations.feedback.up")}</option>
            <option value="down">{t("conversations.feedback.down")}</option>
            <option value="none">{t("conversations.feedback.none")}</option>
          </Select>
        </Field>
        <Field id="conv-sort" label={t("conversations.filter.sort")} className="w-44">
          <Select
            id="conv-sort"
            value={filter.sort ?? "lastAt"}
            onChange={(e) => set({ sort: e.target.value as ConversationFilter["sort"] })}
          >
            <option value="lastAt">{t("conversations.sort.lastAt")}</option>
            <option value="costUsd">{t("conversations.sort.costUsd")}</option>
            <option value="tokens">{t("conversations.sort.tokens")}</option>
            <option value="totalMs">{t("conversations.sort.totalMs")}</option>
          </Select>
        </Field>
        <Field id="conv-deployment" label={t("conversations.filter.deployment")} className="w-56">
          <ResourceNamePicker
            id="conv-deployment"
            labelled
            label={t("conversations.filter.deployment")}
            from={{ project, plural: "assistantdeployments" }}
            value={filter.deployment ?? ""}
            onChange={(deployment) => set({ deployment })}
            empty={t("conversations.filter.noDeployments")}
          />
        </Field>
        <Field id="conv-from" label={t("conversations.filter.from")} className="w-44">
          <Input id="conv-from" type="date" value={filter.from ?? ""} onChange={(e) => set({ from: e.target.value })} />
        </Field>
        <Field id="conv-to" label={t("conversations.filter.to")} className="w-44">
          <Input id="conv-to" type="date" value={filter.to ?? ""} onChange={(e) => set({ to: e.target.value })} />
        </Field>
        <Switch
          checked={Boolean(filter.slow)}
          onCheckedChange={(slow) => set({ slow })}
          label={t("conversations.filter.slow")}
          className="mb-2"
        />
        <Switch
          checked={Boolean(filter.expensive)}
          onCheckedChange={(expensive) => set({ expensive })}
          label={t("conversations.filter.expensive")}
          className="mb-2"
        />
      </div>
      <UsageSummary project={project} filter={shown} />
      {list.isError ? (
        <PageFailed error={list.error} onRetry={() => void list.refetch()}>
          {reasonOf(list.error, t("app.error.generic"))}
        </PageFailed>
      ) : null}
      {list.isLoading ? <p role="status">{t("app.loading")}</p> : null}
      {list.isSuccess && items.length === 0 ? (
        <EmptyState title={t("conversations.empty")} description={t("conversations.emptyHint")} icon="search" />
      ) : null}
      {items.length > 0 ? (
        <ManifestGrid<(typeof items)[number]>
          caption={t("conversations.title")}
          storageKey="conversations"
          searchLabel={t("grid.searchIn", { list: t("conversations.title") })}
          rows={items}
          rowKey={(row) => row.id}
          query={list}
          name={{
            id: "question",
            label: t("conversations.column.question"),
            text: (row) => `${visible(row.firstQuestion)} (${row.deployment})`,
            // The question first: on a phone the first column stays in view while the rest scrolls.
            cell: (row) => (
              <>
                {/* The row's link (T-2875, T-6322): a plain click opens the conversation below, a
                    modified one opens its own address (`?open=`) in a new tab. */}
                <LocalRecordLink
                  href={`?open=${encodeURIComponent(row.id)}`}
                  onOpen={() => setOpen(row.id)}
                  className="text-left underline"
                >
                  {visible(row.firstQuestion)}
                </LocalRecordLink>
                <div className="text-caption text-fg-muted">{row.deployment}</div>
              </>
            ),
          }}
          columns={[
            { id: "lastAt", label: t("conversations.column.lastAt"), text: (row) => row.lastAt, cell: (row) => timeText(row.lastAt, i18n.language) },
            { id: "turns", label: t("conversations.column.turns"), numeric: true, align: "right", text: (row) => String(row.turns) },
            { id: "route", label: t("conversations.column.route"), filter: true, text: (row) => row.routes.join(", ") },
            { id: "firstToken", label: t("conversations.column.firstToken"), numeric: true, align: "right", text: (row) => String(row.firstTokenMs ?? ""), cell: (row) => msText(row.firstTokenMs) },
            { id: "slowest", label: t("conversations.column.slowest"), numeric: true, align: "right", text: (row) => String(row.totalMs ?? ""), cell: (row) => msText(row.totalMs) },
            {
              id: "tokens",
              label: t("conversations.column.tokens"),
              numeric: true,
              align: "right",
              text: (row) => String((row.tokensIn ?? 0) + (row.tokensOut ?? 0)),
              cell: (row) => (
                <>
                  {tokensText(row.tokensIn, i18n.language)} / {tokensText(row.tokensOut, i18n.language)}
                  {row.models && row.models.length > 0 ? <div className="text-caption text-fg-muted">{row.models.join(", ")}</div> : null}
                </>
              ),
            },
            {
              id: "cost",
              label: t("conversations.column.cost"),
              numeric: true,
              align: "right",
              text: (row) => String(row.costUsd),
              cell: (row) => (
                <>
                  {costText(row.costUsd, row.uncosted, t("conversations.unknownPrice"))}
                  {row.uncosted > 0 && row.costUsd > 0 ? (
                    <span className="text-caption text-fg-muted"> {t("conversations.uncosted", { count: row.uncosted })}</span>
                  ) : null}
                </>
              ),
            },
            {
              id: "status",
              label: t("conversations.column.status"),
              filter: true,
              text: (row) => (row.errors > 0 ? "erred" : row.timeouts > 0 ? "timeout" : "ok"),
              valueLabel: (status) => t(`conversations.status.${status}`),
              cell: (row) => (
                <>
                  {row.timeouts > 0 ? <Badge tone="warning">{t("conversations.status.timeout")}</Badge> : null}
                  {row.errors > 0 ? <Badge tone="danger">{t("conversations.status.erred")}</Badge> : null}
                  {row.lastError ? (
                    <span className="block max-w-56 text-caption text-fg-muted [overflow-wrap:anywhere]">{visible(row.lastError)}</span>
                  ) : null}
                  {row.thumbsDown > 0 ? <Badge tone="danger">{t("conversations.feedback.down")}</Badge> : null}
                  {row.thumbsUp > 0 ? <Badge tone="success">{t("conversations.feedback.up")}</Badge> : null}
                  {row.errors + row.timeouts + row.thumbsDown + row.thumbsUp === 0 ? <Badge tone="neutral">{t("conversations.status.ok")}</Badge> : null}
                </>
              ),
            },
          ]}
          empty={<EmptyState bare title={t("conversations.empty")} description={t("conversations.emptyHint")} icon="search" />}
        />
      ) : null}
      {list.data?.nextCursor ? (
        <Button onClick={() => setFilter((now) => ({ ...now, cursor: list.data?.nextCursor ?? undefined }))}>
          {t("conversations.next")}
        </Button>
      ) : null}
      {open ? (
        <ConversationDetail
          key={open}
          project={project}
          id={open}
          against={open === opened ? against : undefined}
          mayErase={can("AssistantDeployment", "delete")}
          onGone={() => setOpen(null)}
        />
      ) : null}
    </section>
  );
}

function ConversationDetail({
  project,
  id,
  against,
  mayErase,
  onGone,
}: {
  project: string;
  id: string;
  against?: string;
  mayErase: boolean;
  onGone: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [confirm, setConfirm] = useState(false);
  const [copied, setCopied] = useState<number | null>(null);
  const [same, setSame] = useState<number | null>(null);
  // `undefined` until a person picks: an eval link's `against` then compares the last turns.
  const [picked, setCompare] = useState<Compare | null | undefined>(undefined);
  const one = useQuery({ queryKey: conversationKeys.one(project, id), queryFn: () => getConversation(project, id) });
  const last = one.data?.turns.at(-1);
  const compare =
    picked !== undefined ? picked : against && last ? { turn: last, other: { conversation: against, turn: null } } : null;
  const erase = useMutation({
    mutationFn: () => eraseConversation(project, id),
    onSuccess: () => {
      setConfirm(false);
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "knowledge", "conversations"] });
      onGone();
    },
  });
  const copy = (turn: ConversationTurn) => {
    void navigator.clipboard
      .writeText(evalCase(turn, i18n.language))
      .then(() => setCopied(turn.turn))
      .catch(() => setCopied(null));
  };
  return (
    <Card aria-label={t("conversations.detail")}>
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-title">{t("conversations.detail")}</h2>
        {mayErase ? (
          <Button variant="danger" onClick={() => setConfirm(true)}>
            {t("conversations.erase")}
          </Button>
        ) : null}
      </div>
      {one.isError ? <Alert tone="danger">{reasonOf(one.error, t("app.error.generic"))}</Alert> : null}
      {one.isLoading ? <p role="status">{t("app.loading")}</p> : null}
      {copied !== null ? <Alert role="status" tone="success">{t("conversations.copied")}</Alert> : null}
      <ol className="space-y-6">
        {(one.data?.turns ?? []).map((turn) => (
          <li key={turn.turn} className="space-y-2">
            <div className="flex flex-wrap items-center gap-2 text-caption text-fg-muted">
              <span>{timeText(turn.at, i18n.language)}</span>
              <Badge tone="neutral">{turn.route}</Badge>
              <span>{t("conversations.timing", { first: msText(turn.firstTokenMs), total: msText(turn.totalMs) })}</span>
              <span>{usdText(turn.costUsd)}</span>
              {turn.timedOut ? <Badge tone="warning">{t("conversations.status.timeout")}</Badge> : null}
              {turn.verdict === "down" ? <Badge tone="danger">{t("conversations.feedback.down")}</Badge> : null}
              {turn.verdict === "up" ? <Badge tone="success">{t("conversations.feedback.up")}</Badge> : null}
              <Button size="sm" onClick={() => copy(turn)}>
                {t("conversations.copyAsTest")}
              </Button>
              <Button size="sm" variant="secondary" onClick={() => setSame(same === turn.turn ? null : turn.turn)}>
                {t("conversations.same")}
              </Button>
            </div>
            <p className="font-semibold">{visible(turn.question)}</p>
            <p className="whitespace-pre-wrap">{visible(turn.answer)}</p>
            {turn.error ? <Alert tone="danger">{visible(turn.error.detail ?? turn.error.title ?? "")}</Alert> : null}
            {same === turn.turn ? (
              <SameList
                project={project}
                id={id}
                turn={turn.turn}
                onCompare={(other) => setCompare({ turn, other })}
              />
            ) : null}
            {compare?.turn.turn === turn.turn ? (
              <CompareView project={project} left={turn} other={compare.other} onClose={() => setCompare(null)} />
            ) : null}
            <Steps steps={turn.steps} />
          </li>
        ))}
      </ol>
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={t("conversations.eraseTitle")}
        description={t("conversations.eraseHint")}
        confirmLabel={t("conversations.erase")}
        tone="danger"
        pending={erase.isPending}
        onConfirm={() => erase.mutate()}
      >
        {erase.isError ? <Alert tone="danger">{reasonOf(erase.error, t("app.error.generic"))}</Alert> : null}
      </ConfirmDialog>
    </Card>
  );
}

/** The steps of one turn as a timeline: when each started, what it was, and what it cost. */
function Steps({ steps }: { steps: ConversationStep[] }): JSX.Element | null {
  const { t } = useTranslation();
  if (steps.length === 0) return null;
  return (
    <ol aria-label={t("conversations.steps")} className="space-y-1 border-l border-border pl-3 text-caption">
      {steps.map((step, i) => (
        <li key={i}>
          <details>
            <summary className="cursor-pointer">
              <span className="tabular-nums text-fg-muted">+{msText(step.startMs)}</span>{" "}
              <strong>{t(`conversations.kind.${step.kind}`)}</strong> {step.model ?? step.name}
              {step.endpoint ? ` · ${step.endpoint}` : ""} · {msText(step.durationMs)}
              {step.tokensIn !== null ? ` · ${step.tokensIn}/${step.tokensOut ?? 0} tok` : ""}
              {step.cachedTokens ? ` · ${t("conversations.cached", { count: step.cachedTokens })}` : ""}
              {step.rows !== null && step.rows !== undefined ? ` · ${t("conversations.rows", { count: step.rows })}` : ""}
              {step.kind === "model" ? ` · ${step.costUsd === null ? t("conversations.unknownPrice") : usdText(step.costUsd)}` : ""}
              {step.status === "failed" ? <Badge tone="danger">{t("conversations.unfinished")}</Badge> : null}
            </summary>
            <StepBody step={step} />
          </details>
        </li>
      ))}
    </ol>
  );
}

/** A step's stored text: arguments, result, reasoning and error, all as inert text. */
function StepBody({ step }: { step: ConversationStep }): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      {step.input ? <StoredText text={step.input} /> : null}
      {step.output ? <StoredText text={step.output} /> : null}
      {step.reasoning ? (
        <>
          <p className="mt-1 font-semibold">{t("conversations.reasoning")}</p>
          <StoredText text={step.reasoning} />
        </>
      ) : null}
      {step.error ? <p className="text-danger">{visible(step.error)}</p> : null}
    </>
  );
}

/**
 * Stored text as React text, never markup: bidi and zero-width characters shown as markers, and
 * the first 64 KB until "Zobraziť viac" (T-4681).
 */
function StoredText({ text }: { text: string }): JSX.Element {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  const shown = capped(text, all ? Infinity : undefined);
  return (
    <>
      <CodeBlock label={t("codeBlock.text")} className="mt-1 max-h-64 whitespace-pre-wrap bg-surface-subtle p-2">{visible(shown.text)}</CodeBlock>
      {shown.more ? (
        <Button size="sm" variant="ghost" onClick={() => setAll(true)}>
          {t("conversations.showMore", { size: Math.round(text.length / 1024) })}
        </Button>
      ) : null}
    </>
  );
}

/** "Rovnaká otázka": every stored turn whose question reads the same, newest first (T-4681). */
function SameList({
  project,
  id,
  turn,
  onCompare,
}: {
  project: string;
  id: string;
  turn: number;
  onCompare: (other: { conversation: string; turn: number }) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const same = useQuery({ queryKey: conversationKeys.same(project, id, turn), queryFn: () => sameQuestion(project, id, turn) });
  if (same.isError) return <Alert tone="danger">{reasonOf(same.error, t("app.error.generic"))}</Alert>;
  if (same.isLoading) return <p role="status">{t("app.loading")}</p>;
  const others = (same.data ?? []).filter((row) => row.conversation !== id || row.turn !== turn);
  if (others.length === 0) return <p className="text-caption text-fg-muted">{t("conversations.sameEmpty")}</p>;
  return (
    <ul aria-label={t("conversations.same")} className="space-y-1 text-caption">
      {others.map((row) => (
        <li key={`${row.conversation}/${row.turn}`} className="flex flex-wrap items-center gap-2">
          <span>{timeText(row.at, i18n.language)}</span>
          <span>{row.deployment}</span>
          <span>{msText(row.totalMs)}</span>
          <span>{usdText(row.costUsd)}</span>
          <span>{t("conversations.stepCount", { count: row.steps })}</span>
          <Button size="sm" onClick={() => onCompare({ conversation: row.conversation, turn: row.turn })}>
            {t("conversations.compare")}
          </Button>
        </li>
      ))}
    </ul>
  );
}

/** Two turns to compare: a turn of this conversation, and another's turn (null: its last). */
interface Compare {
  turn: ConversationTurn;
  other: { conversation: string; turn: number | null };
}

/**
 * Two turns side by side (T-4681): the totals, the visuals' spec changes, the steps aligned by
 * kind with their arguments, results and reasoning, and a word diff of the answers.
 */
function CompareView({
  project,
  left,
  other,
  onClose,
}: {
  project: string;
  left: ConversationTurn;
  other: Compare["other"];
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const loaded = useQuery({
    queryKey: conversationKeys.one(project, other.conversation),
    queryFn: () => getConversation(project, other.conversation),
  });
  const turns = loaded.data?.turns ?? [];
  const right = other.turn === null ? turns.at(-1) : turns.find((turn) => turn.turn === other.turn);
  const copy = (pair: [ConversationTurn, ConversationTurn]) => {
    void navigator.clipboard
      .writeText(JSON.stringify(pair, null, 2))
      .then(() => setCopied(true))
      .catch(() => setCopied(false));
  };
  return (
    <section aria-label={t("conversations.compareTitle")} className="space-y-3 rounded border border-border p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-semibold">{t("conversations.compareTitle")}</h3>
        <div className="flex gap-2">
          {right ? (
            <Button size="sm" onClick={() => copy([left, right])}>
              {t("conversations.copyPair")}
            </Button>
          ) : null}
          <Button size="sm" variant="ghost" onClick={onClose}>
            {t("conversations.compareClose")}
          </Button>
        </div>
      </div>
      {copied ? <Alert role="status" tone="success">{t("conversations.copied")}</Alert> : null}
      {loaded.isError ? <Alert tone="danger">{reasonOf(loaded.error, t("app.error.generic"))}</Alert> : null}
      {loaded.isLoading ? <p role="status">{t("app.loading")}</p> : null}
      {loaded.isSuccess && !right ? <Alert tone="info">{t("conversations.sameEmpty")}</Alert> : null}
      {right ? <Compared left={left} right={right} /> : null}
    </section>
  );
}

function Compared({ left, right }: { left: ConversationTurn; right: ConversationTurn }): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      <Totals left={left} right={right} />
      <ChangeList changes={visualChanges(left.steps, right.steps)} />
      <ol aria-label={t("conversations.steps")} className="space-y-1 text-caption">
        {aligned(left.steps, right.steps).map(([a, b], i) => (
          <li key={i} className="grid gap-2 border-l border-border pl-3 md:grid-cols-2">
            <ComparedStep step={a} side="left" />
            <ComparedStep step={b} side="right" />
          </li>
        ))}
      </ol>
      <AnswerDiff left={left.answer} right={right.answer} />
    </>
  );
}

/** The header lines of the two compared turns, side by side. */
function Totals({ left, right }: { left: ConversationTurn; right: ConversationTurn }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="grid gap-2 text-caption md:grid-cols-2">
      <p data-testid="compare-left-totals">{totalsText(t, left)}</p>
      <p data-testid="compare-right-totals">{totalsText(t, right)}</p>
    </div>
  );
}

/** A compared turn's header line: steps, tool and model calls, time, cost and cached share. */
function totalsText(t: TFunction, turn: ConversationTurn): string {
  const sum = totals(turn);
  return t("conversations.totals", {
    steps: sum.steps,
    tools: sum.tools,
    models: sum.models,
    time: msText(sum.totalMs),
    cost: usdText(sum.costUsd),
    cached: sum.cachedShare === null ? "–" : `${Math.round(sum.cachedShare * 100)} %`,
  });
}

/** The spec changes between the two turns' tables, charts and maps. */
function ChangeList({ changes }: { changes: string[] }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="text-caption">
      <p className="font-semibold">{t("conversations.changes")}</p>
      {changes.length === 0 ? (
        <p className="text-fg-muted">{t("conversations.noChanges")}</p>
      ) : (
        <ul aria-label={t("conversations.changes")} className="list-disc pl-5">
          {changes.map((change, i) => (
            <li key={i}>{visible(change)}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** One side of an aligned row: the step, or an empty cell where the other turn had one. */
function ComparedStep({ step, side }: { step: ConversationStep | null; side: "left" | "right" }): JSX.Element {
  const { t } = useTranslation();
  if (!step) return <span data-side={side} className="text-fg-muted">–</span>;
  return (
    <details data-side={side}>
      <summary className="cursor-pointer">
        <strong>{t(`conversations.kind.${step.kind}`)}</strong> {visible(step.model ?? step.name)}
        {step.endpoint ? ` · ${visible(step.endpoint)}` : ""} · {msText(step.durationMs)}
        {step.tokensIn !== null ? ` · ${step.tokensIn}/${step.tokensOut ?? 0} tok` : ""}
        {step.kind === "model" ? ` · ${usdText(step.costUsd)}` : ""}
      </summary>
      <StepBody step={step} />
    </details>
  );
}

/** The two answers as a word diff: what the second dropped struck out, what it added marked. */
function AnswerDiff({ left, right }: { left: string; right: string }): JSX.Element {
  const { t } = useTranslation();
  const word = (w: DiffWord, i: number) =>
    w.change === "same" ? <span key={i}>{visible(w.word)} </span>
    : w.change === "removed" ? <del key={i} className="text-danger">{visible(w.word)} </del>
    : <ins key={i} className="text-success">{visible(w.word)} </ins>;
  return (
    <div>
      <p className="text-caption font-semibold">{t("conversations.answers")}</p>
      <p className="whitespace-pre-wrap" data-testid="answer-diff">
        {wordDiff(left, right).map(word)}
      </p>
    </div>
  );
}
