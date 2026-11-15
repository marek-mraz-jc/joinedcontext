import { lazy, Suspense, useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { api, queryKeys } from "../../api/client";
import { asManifests, isChange } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { allows, usePermissions, useReadableList } from "../../api/permissions";
import { importManifests, proposeChecked } from "../../api/proposal";
import type { ResourceProposal } from "../../api/manifest";
import { Alert, Badge, Button, Card, Checkbox, CodeBlock, EmptyState, Field, Input, Select, Skeleton, Textarea } from "../../components/ui";
import { AdminGrid } from "../../components/grid/AdminGrid";
import type { DataColumn } from "../../components/grid/AdminGrid";
import { ChangeNotice } from "../../components/ChangeNotice";
import { KindList } from "../../routes/ResourceListPage";
import {
  digest,
  MAX_CODE_BYTES,
  movedPins,
  newest,
  specOf,
  suggestVersion,
  usedBy,
  versionProblem,
  withVersion,
} from "./scriptDraft";
import type { ScriptTest } from "./scriptDraft";

const ScriptCodeMonaco = lazy(() => import("./ScriptCodeMonaco"));

const NEW = "";
const STARTER = "const record = bento.v0_msg_as_structured();\nrecord.checked = true;\nbento.v0_msg_set_structured(record);\n";
const STARTER_TESTS = '[\n  { "name": "marks-one", "input": { "id": "a" }, "expected": { "id": "a", "checked": true } }\n]';
const NAME = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

export interface TestResult {
  name: string;
  ok: boolean;
  output?: unknown;
  error?: string;
  millis: number;
}

/** One run of tests on the project's runner (PL-118): the results, or the refusal in words. */
async function runTests(
  project: string,
  code: string,
  tests: ScriptTest[],
  limits: { timeoutMs?: number; memoryMb?: number },
  silent: string,
): Promise<TestResult[]> {
  const { data, error } = await api.POST("/api/v1/projects/{project}/scripts/test", {
    params: { path: { project } },
    body: { code, limits, tests },
  });
  if (data) return (data as unknown as { results: TestResult[] }).results;
  const detail = (error as { detail?: unknown } | undefined)?.detail;
  throw new Error(typeof detail === "string" ? detail : silent);
}

function parseTests(text: string): ScriptTest[] | null {
  try {
    const parsed: unknown = JSON.parse(text);
    if (!Array.isArray(parsed)) return null;
    return parsed.every((t) => t && typeof t === "object" && typeof (t as ScriptTest).name === "string" && "input" in t && "expected" in t)
      ? (parsed as ScriptTest[])
      : null;
  } catch {
    return null;
  }
}

/** A limit typed in the form: empty is the runner's default; otherwise a whole number in 1..max. */
function limitOf(text: string, max: number): number | undefined | null {
  if (text.trim() === "") return undefined;
  const value = Number(text);
  return Number.isInteger(value) && value >= 1 && value <= max ? value : null;
}

/**
 * The project's stored scripts (PL-118…PL-121, T-7084): the list, a workbench that edits the next
 * version, runs its tests on the runner and proposes it, and who pins each script with a move of
 * their pins. Every proposal is a Change a second person approves.
 */
export function ScriptsPage({ project }: { project: string }): JSX.Element {
  const scripts = useReadableList(project, "scripts", "Script");
  const pipelines = useReadableList(project, "pipelines", "Pipeline");
  const items = asManifests(scripts.data?.items ?? []);
  const [selected, setSelected] = useState<string>(NEW);
  const stored = items.find((m) => m.metadata.name === selected);
  return (
    <div className="flex flex-col gap-section">
      <KindList project={project} plural="scripts" />
      {scripts.isLoading ? (
        <Skeleton className="h-64" />
      ) : (
        <Workbench
          key={selected}
          project={project}
          scripts={items}
          selected={selected}
          onSelect={setSelected}
          stored={stored}
        />
      )}
      {stored ? (
        <UsedBy project={project} script={stored} pipelines={asManifests(pipelines.data?.items ?? [])} />
      ) : null}
    </div>
  );
}

function Workbench({
  project,
  scripts,
  selected,
  onSelect,
  stored,
}: {
  project: string;
  scripts: Manifest[];
  selected: string;
  onSelect: (name: string) => void;
  stored: Manifest | undefined;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const permissions = usePermissions(project);
  const mayPropose = allows(permissions.data, "Script", "propose");
  const spec = specOf(stored);
  const base = newest(spec);
  const [name, setName] = useState("");
  const [code, setCode] = useState(base?.code ?? STARTER);
  const [testsText, setTestsText] = useState(base?.tests ? JSON.stringify(base.tests, null, 2) : STARTER_TESTS);
  const [timeout, setTimeoutText] = useState(base?.limits?.timeoutMs?.toString() ?? "");
  const [memory, setMemory] = useState(base?.limits?.memoryMb?.toString() ?? "");
  const [results, setResults] = useState<TestResult[] | null>(null);
  const [changed, setChanged] = useState<string[]>([]);
  const [version, setVersion] = useState(suggestVersion(spec, true));
  const [running, setRunning] = useState(false);
  const [proposing, setProposing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [proposed, setProposed] = useState<{ version: string; change: Change | null } | null>(null);

  const tests = parseTests(testsText);
  const timeoutMs = limitOf(timeout, 200);
  const memoryMb = limitOf(memory, 64);
  const scriptName = stored ? stored.metadata.name : name;
  const nameWrong = !stored && !NAME.test(name);
  const codeWrong = code.trim() === "" || new TextEncoder().encode(code).length > MAX_CODE_BYTES;
  const problem = versionProblem(spec, version);
  const edited = () => {
    setResults(null);
    setProposed(null);
  };

  const run = async () => {
    if (!tests || timeoutMs === null || memoryMb === null) return;
    setRunning(true);
    setError(null);
    try {
      const limits = { timeoutMs, memoryMb };
      const now = await runTests(project, code, tests, limits, t("scripts.reason.runner"));
      // The outputs the newest version makes of the same inputs: what this edit changes (G-26).
      const before = base ? await runTests(project, base.code, tests, base.limits ?? {}, t("scripts.reason.runner")) : [];
      const differs = now
        .filter((r, i) => base && JSON.stringify(r.output) !== JSON.stringify(before[i]?.output))
        .map((r) => r.name);
      setResults(now);
      setChanged(differs);
      setVersion(suggestVersion(spec, differs.length > 0 || !base));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setRunning(false);
    }
  };

  const failing = results?.filter((r) => !r.ok).length ?? 0;
  const blocked = proposed
    ? t("scripts.reason.proposed")
    : error && results === null
      ? t("scripts.reason.runFailed")
      : nameWrong
        ? t("scripts.reason.name")
        : codeWrong
          ? t("scripts.reason.code")
          : !tests
            ? t("scripts.reason.tests")
            : results === null
              ? t("scripts.reason.run")
              : failing > 0
                ? t("scripts.reason.failing", { count: failing })
                : problem === "format"
                  ? t("scripts.reason.versionFormat")
                  : problem === "order"
                    ? t("scripts.reason.versionOrder", { version: base?.version ?? "" })
                    : undefined;

  const propose = async () => {
    if (blocked || !tests) return;
    setProposing(true);
    setError(null);
    try {
      const manifest = withVersion(project, scriptName, stored, {
        version,
        state: "published",
        code,
        sha256: await digest(code),
        ...(timeoutMs || memoryMb ? { limits: { ...(timeoutMs ? { timeoutMs } : {}), ...(memoryMb ? { memoryMb } : {}) } } : {}),
        tests,
      });
      const answer = await proposeChecked(project, "scripts", manifest as unknown as ResourceProposal, !stored);
      await queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "scripts") });
      setProposed({ version, change: isChange(answer) ? answer : null });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setProposing(false);
    }
  };

  // Running tests needs the same right as proposing (PL-118): without it the form has nothing to offer.
  if (!mayPropose) {
    return (
      <Card role="region" aria-labelledby="script-workbench-title">
        <div className="flex flex-col gap-stack">
          <div>
            <h2 id="script-workbench-title" className="text-title font-semibold">
              {t("scripts.workbench.title")}
            </h2>
          </div>
          <Alert tone="info">{t("scripts.reason.rightsNext")}</Alert>
        </div>
      </Card>
    );
  }

  return (
    <Card role="region" aria-labelledby="script-workbench-title">
      <div className="flex flex-col gap-stack">
        <div>
          <h2 id="script-workbench-title" className="text-title font-semibold">
            {t("scripts.workbench.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("scripts.workbench.purpose")}</p>
        </div>
        <Field id="script-pick" label={t("scripts.field.script")} help={t("scripts.hint.script")}>
          <Select id="script-pick" value={selected} onChange={(e) => onSelect(e.target.value)}>
            <option value={NEW}>{t("scripts.field.newScript")}</option>
            {scripts.map((s) => (
              <option key={s.metadata.name} value={s.metadata.name}>
                {s.metadata.name}
              </option>
            ))}
          </Select>
        </Field>
        {scripts.length === 0 ? <p className="text-body text-fg-muted">{t("scripts.empty")}</p> : null}
        {stored ? null : (
          <Field
            id="script-name"
            label={t("scripts.field.name")}
            required
            help={t("scripts.hint.name")}
            errors={name !== "" && nameWrong ? [t("scripts.reason.name")] : undefined}
          >
            <Input id="script-name" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
        )}
        {/* Monaco owns its textbox: a Field would put the label's wiring on the wrapper (axe aria-allowed-attr). */}
        <div className="flex flex-col gap-1.5">
          <span id="script-code-label" className="text-body font-medium text-fg">
            {t("scripts.field.code")}
          </span>
          <div className="overflow-hidden rounded-md border border-border">
            <Suspense fallback={<Skeleton className="h-60" />}>
              <ScriptCodeMonaco
                value={code}
                onChange={(next) => {
                  setCode(next);
                  edited();
                }}
                ariaLabel={t("scripts.field.code")}
                describedBy="script-code-hint"
              />
            </Suspense>
          </div>
          <p id="script-code-hint" className="text-caption text-fg-muted break-words">
            {t("scripts.hint.code")}
          </p>
        </div>
        <Field
          id="script-tests"
          label={t("scripts.field.tests")}
          required
          help={t("scripts.hint.tests")}
          errors={tests ? undefined : [t("scripts.reason.tests")]}
        >
          <Textarea
            id="script-tests"
            rows={10}
            className="font-mono"
            value={testsText}
            onChange={(e) => {
              setTestsText(e.target.value);
              edited();
            }}
          />
        </Field>
        <div className="grid max-w-xl gap-stack sm:grid-cols-2">
          <Field
            id="script-timeout"
            label={t("scripts.field.timeout")}
            help={t("scripts.hint.timeout")}
            errors={timeoutMs === null ? [t("scripts.reason.timeout")] : undefined}
          >
            <Input id="script-timeout" inputMode="numeric" value={timeout} placeholder="50" onChange={(e) => setTimeoutText(e.target.value)} />
          </Field>
          <Field
            id="script-memory"
            label={t("scripts.field.memory")}
            help={t("scripts.hint.memory")}
            errors={memoryMb === null ? [t("scripts.reason.memory")] : undefined}
          >
            <Input id="script-memory" inputMode="numeric" value={memory} placeholder="32" onChange={(e) => setMemory(e.target.value)} />
          </Field>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            loading={running}
            onClick={() => void run()}
            disabled={!tests || codeWrong || timeoutMs === null || memoryMb === null}
            disabledReason={!tests ? t("scripts.reason.tests") : codeWrong ? t("scripts.reason.code") : t("scripts.reason.limits")}
          >
            {t("scripts.action.run")}
          </Button>
        </div>
        {error ? <Alert tone="danger">{error}</Alert> : null}
        {results && base && changed.length > 0 ? (
          <p className="text-body text-fg-muted">{t("scripts.results.changedNote", { version: base.version })}</p>
        ) : null}
        {results ? <TestResults results={results} changed={changed} /> : null}
        <Field
          id="script-version"
          label={t("scripts.field.version")}
          required
          help={t("scripts.hint.version", { version: suggestVersion(spec, changed.length > 0 || !base) })}
        >
          <Input id="script-version" value={version} onChange={(e) => setVersion(e.target.value)} />
        </Field>
        <div className="flex flex-wrap items-center gap-2">
          <Button loading={proposing} onClick={() => void propose()} disabled={Boolean(blocked)} disabledReason={blocked}>
            {t("scripts.action.propose", { version })}
          </Button>
          {blocked ? <span className="text-caption text-fg-muted">{blocked}</span> : null}
        </div>
        {proposed ? (
          <>
            {proposed.change ? <ChangeNotice change={proposed.change} project={project} /> : null}
            <p className="text-body text-fg-muted">{t("scripts.proposed", { version: proposed.version })}</p>
          </>
        ) : null}
      </div>
    </Card>
  );
}

function TestResults({ results, changed }: { results: TestResult[]; changed: string[] }): JSX.Element {
  const { t } = useTranslation();
  const columns: DataColumn<TestResult>[] = [
    {
      key: "name",
      label: t("scripts.results.test"),
      text: (r) => r.name,
      cell: (r) => (
        <>
          {r.name}
          {changed.includes(r.name) ? (
            <Badge tone="info" className="ml-2">
              {t("scripts.results.changed")}
            </Badge>
          ) : null}
        </>
      ),
    },
    {
      key: "verdict",
      label: t("scripts.results.verdict"),
      text: (r) => (r.ok ? t("scripts.results.pass") : t("scripts.results.fail")),
      cell: (r) => <Badge tone={r.ok ? "success" : "danger"}>{r.ok ? t("scripts.results.pass") : t("scripts.results.fail")}</Badge>,
    },
    {
      key: "output",
      label: t("scripts.results.output"),
      filter: false,
      text: (r) => r.error ?? JSON.stringify(r.output),
      cell: (r) =>
        r.error ? (
          <span className="text-danger-fg">{r.error}</span>
        ) : (
          <CodeBlock label={t("scripts.results.outputOf", { test: r.name })}>{JSON.stringify(r.output, null, 2)}</CodeBlock>
        ),
    },
    { key: "time", label: t("scripts.results.time"), align: "right", text: (r) => t("scripts.results.millis", { count: r.millis }) },
  ];
  return (
    <AdminGrid
      stack
      noSearch
      storageKey="scripts-results"
      caption={t("scripts.results.caption")}
      rows={results}
      columns={columns}
      rowKey={(r) => r.name}
      empty={<EmptyState bare title={t("scripts.results.emptyTitle")} description={t("scripts.results.empty")} />}
    />
  );
}

function UsedBy({ project, script, pipelines }: { project: string; script: Manifest; pipelines: Manifest[] }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const permissions = usePermissions(project);
  const name = script.metadata.name;
  const uses = useMemo(() => usedBy(pipelines, name), [pipelines, name]);
  const published = (specOf(script)?.versions ?? []).filter((v) => v.state === "published").map((v) => v.version);
  const [target, setTarget] = useState(published[published.length - 1] ?? "");
  const [chosen, setChosen] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ tone: "success" | "danger"; text: string; change?: Change } | null>(null);
  const mayPropose = allows(permissions.data, "Pipeline", "propose");
  const movable = uses.filter((u) => u.versions.some((v) => v !== target));
  const picked = chosen.filter((p) => movable.some((u) => u.pipeline === p));
  const reason = !mayPropose ? t("scripts.reason.pipelineRights") : picked.length === 0 ? t("scripts.reason.pick") : undefined;

  const usedColumns: DataColumn<(typeof uses)[number]>[] = [
    {
      key: "pipeline",
      label: t("scripts.used.pipeline"),
      text: (u) => u.pipeline,
      cell: (u) => (
        <Checkbox
          label={u.pipeline}
          checked={chosen.includes(u.pipeline)}
          disabled={!movable.some((m) => m.pipeline === u.pipeline)}
          disabledReason={t("scripts.used.already", { version: target })}
          onChange={(e) => setChosen((now) => (e.target.checked ? [...now, u.pipeline] : now.filter((p) => p !== u.pipeline)))}
        />
      ),
    },
    {
      key: "pinned",
      label: t("scripts.used.pinned"),
      text: (u) => u.versions.join(", "),
      cell: (u) => (
        <>
          {u.versions.join(", ")}
          {movable.some((m) => m.pipeline === u.pipeline) ? null : (
            <Badge tone="neutral" className="ml-2">
              {t("scripts.used.onTarget")}
            </Badge>
          )}
        </>
      ),
    },
  ];

  const move = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const manifests = pipelines.filter((p) => picked.includes(p.metadata.name)).map((p) => movedPins(p, name, target));
      // One Change for the project, its dry run first (PF-57).
      await importManifests(project, manifests, true);
      const answer = await importManifests(project, manifests, false);
      await queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "pipelines") });
      setMessage({
        tone: "success",
        text: t("scripts.used.moved", { names: new Intl.ListFormat(i18n.language, { type: "conjunction" }).format(picked), version: target }),
        ...(isChange(answer) ? { change: answer } : {}),
      });
      setChosen([]);
    } catch (e) {
      setMessage({ tone: "danger", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card role="region" aria-labelledby="script-used-title">
      <div className="flex flex-col gap-stack">
        <h2 id="script-used-title" className="text-title font-semibold">
          {t("scripts.used.title", { name })}
        </h2>
        {uses.length === 0 ? (
          <EmptyState bare title={t("scripts.used.emptyTitle")} description={t("scripts.used.empty", { name })} />
        ) : (
          <>
            <AdminGrid
              stack
              storageKey={`${project}:script-used`}
              caption={t("scripts.used.caption", { name })}
              rows={uses}
              columns={usedColumns}
              rowKey={(u) => u.pipeline}
              empty={<EmptyState bare title={t("scripts.used.emptyTitle")} description={t("scripts.used.empty", { name })} />}
            />
            <Field id="script-target" label={t("scripts.field.target")} help={t("scripts.hint.target")}>
              <Select id="script-target" value={target} onChange={(e) => setTarget(e.target.value)}>
                {published.map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </Select>
            </Field>
            <div className="flex flex-wrap items-center gap-2">
              <Button loading={busy} onClick={() => void move()} disabled={Boolean(reason)} disabledReason={reason}>
                {t("scripts.action.move", { count: picked.length, version: target })}
              </Button>
              {reason ? <span className="text-caption text-fg-muted">{reason}</span> : null}
            </div>
          </>
        )}
        {message?.change ? <ChangeNotice change={message.change} project={project} /> : null}
        {message?.tone === "success" ? <p className="text-body text-fg-muted">{message.text}</p> : null}
        {message?.tone === "danger" ? <Alert tone="danger">{message.text}</Alert> : null}
      </div>
    </Card>
  );
}
