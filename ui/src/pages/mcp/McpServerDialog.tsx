import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { useProjects } from "../../api/projects";
import { useProposal } from "../../api/proposal";
import { Combobox } from "../../components/pickers/Combobox";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Dialog,
  Field,
  InlineError,
  Input,
  Select,
  Textarea,
} from "../../components/ui";
import { ToolPreview } from "./McpServerPanels";
import { knowledgeKeys, listSources } from "../knowledge/knowledge";
import { AUDIENCES, MAX_KNOWLEDGE_SOURCES, MAX_MEMBERS, breadth, emptyForm, formProblems, fromManifest, memberKey, toManifest, widestAllowed } from "./mcp";
import type { Audience, McpServerForm, MemberChoice } from "./mcp";

/**
 * Creates or edits one named MCP server (MF-53, T-3156): its members picked from the Endpoints
 * the person may read, its audience no wider than the narrowest member, the merged tool list
 * shown before anything is proposed, and the manifest proposed as a checked Change.
 */
export function McpServerDialog({
  project,
  stored,
  choices,
  choicesLoading,
  open,
  onOpenChange,
}: {
  project: string;
  /** The server being edited; none creates one. */
  stored?: Manifest;
  /** Every Endpoint the person may read, across the projects they see. */
  choices: MemberChoice[];
  choicesLoading: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [form, setForm] = useState<McpServerForm>(() => (stored ? fromManifest(project, stored) : emptyForm()));
  const [search, setSearch] = useState("");
  const [tried, setTried] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const proposal = useProposal(project, "mcpservers", setChange);

  const byKey = useMemo(() => new Map(choices.map((choice) => [memberKey(choice), choice])), [choices]);
  const picked = form.members.map((key) => byKey.get(key)).filter((choice): choice is MemberChoice => Boolean(choice));
  // A stored member this person cannot read stays in the manifest, unseen and unchanged (SP-20).
  const unseen = form.members.filter((key) => !byKey.has(key));
  const problems = formProblems(form, picked);
  const widest = widestAllowed(picked);
  const needle = search.trim().toLowerCase();
  const shown = choices.filter(
    (choice) => !needle || memberKey(choice).toLowerCase().includes(needle) || choice.title.toLowerCase().includes(needle),
  );

  // The project's public sources may be attached (MF-57); one of another project, or one this
  // person cannot list, stays in the manifest as it is.
  const sources = useQuery({ queryKey: knowledgeKeys.sources(project), queryFn: () => listSources(project), enabled: open, retry: false });
  const attachable = (sources.data ?? []).filter((row) => row.visibility === "public").map((row) => row.source);
  const keptSources = form.knowledgeSources.filter((name) => !attachable.includes(name));

  const set = (patch: Partial<McpServerForm>) => setForm((current) => ({ ...current, ...patch }));
  const toggle = (key: string, on: boolean) =>
    set({ members: on ? [...form.members, key] : form.members.filter((member) => member !== key) });
  const close = (next: boolean) => {
    if (!next) {
      setChange(null);
      setTried(false);
      proposal.reset();
      if (!stored) setForm(emptyForm());
    }
    onOpenChange(next);
  };
  const submit = () => {
    setTried(true);
    if (Object.keys(problems).length > 0) return;
    proposal.mutation.mutate({ body: toManifest(project, form, stored), create: !stored });
  };
  const shownError = (field: string) => (tried && problems[field] ? [t(problems[field], { max: MAX_MEMBERS })] : undefined);
  // The server checks each admitted project again (T-4948): its refusal shows beside the field.
  const failure = proposal.mutation.error;
  const refusedProjects =
    failure instanceof ApiError && failure.problem?.errors?.includes("spec.allowedProjects") ? proposal.error : null;
  const projects = useProjects();

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="xl"
      title={stored ? t("mcp.form.editTitle", { name: stored.metadata.name }) : t("mcp.form.newTitle")}
      description={t("mcp.form.lead")}
      closeLabel={t("app.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("app.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button loading={proposal.mutation.isPending} onClick={submit}>
              {t("mcp.form.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          <div className="grid gap-4 md:grid-cols-2">
            <Field id="mcp-name" label={t("mcp.field.name")} required help={t("mcp.help.name")} errors={shownError("name")}>
              <Input
                id="mcp-name"
                value={form.name}
                readOnly={Boolean(stored)}
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => set({ name: event.target.value.trim() })}
              />
            </Field>
            <Field id="mcp-title" label={t("mcp.field.title")} help={t("mcp.help.title")}>
              <Input id="mcp-title" value={form.title} maxLength={120} onChange={(event) => set({ title: event.target.value })} />
            </Field>
          </div>
          <Field id="mcp-description" label={t("mcp.field.description")} help={t("mcp.help.description")}>
            <Textarea
              id="mcp-description"
              rows={2}
              maxLength={1000}
              value={form.description}
              onChange={(event) => set({ description: event.target.value })}
            />
          </Field>

          <fieldset className="flex flex-col gap-2">
            <legend className="text-body font-semibold">{t("mcp.field.members", { count: form.members.length, max: MAX_MEMBERS })}</legend>
            <p className="text-caption text-fg-muted">{t("mcp.help.members")}</p>
            <Field id="mcp-member-search" label={t("mcp.field.search")}>
              <Input id="mcp-member-search" type="search" value={search} onChange={(event) => setSearch(event.target.value)} />
            </Field>
            {choicesLoading ? <p role="status">{t("app.loading")}</p> : null}
            {!choicesLoading && shown.length === 0 ? <p>{t("mcp.form.noEndpoints")}</p> : null}
            <ul className="flex max-h-72 flex-col gap-1 overflow-y-auto rounded-md border border-border p-2">
              {shown.map((choice) => {
                const key = memberKey(choice);
                const on = form.members.includes(key);
                const full = !on && form.members.length >= MAX_MEMBERS;
                return (
                  <li key={key} className="flex flex-wrap items-center justify-between gap-2">
                    <Checkbox
                      label={`${choice.title} (${key})`}
                      checked={on}
                      disabled={!choice.servesMcp || full}
                      disabledReason={!choice.servesMcp ? t("mcp.form.servesNoMcp") : full ? t("mcp.form.full", { max: MAX_MEMBERS }) : undefined}
                      onChange={(event) => toggle(key, event.target.checked)}
                    />
                    <Badge tone={choice.audience === "public" ? "accent" : "neutral"}>{t(`mcp.audience.${choice.audience}`)}</Badge>
                  </li>
                );
              })}
            </ul>
            {unseen.length > 0 ? <p className="text-caption text-fg-muted">{t("mcp.form.unseen", { count: unseen.length })}</p> : null}
            {shownError("members") ? (
              <InlineError>
                {shownError("members")?.[0]}
              </InlineError>
            ) : null}
          </fieldset>

          <fieldset className="flex flex-col gap-2">
            <legend className="text-body font-semibold">
              {t("mcp.field.knowledgeSources", { count: form.knowledgeSources.length, max: MAX_KNOWLEDGE_SOURCES })}
            </legend>
            <p className="text-caption text-fg-muted">{t("mcp.help.knowledgeSources")}</p>
            {sources.isLoading ? <p role="status">{t("app.loading")}</p> : null}
            {sources.isError ? <p>{t("mcp.form.sourcesUnreadable")}</p> : null}
            {sources.isSuccess && attachable.length === 0 ? <p>{t("mcp.form.noPublicSources")}</p> : null}
            {attachable.length > 0 ? (
              <ul className="flex flex-col gap-1 rounded-md border border-border p-2">
                {attachable.map((name) => {
                  const on = form.knowledgeSources.includes(name);
                  const full = !on && form.knowledgeSources.length >= MAX_KNOWLEDGE_SOURCES;
                  return (
                    <li key={name}>
                      <Checkbox
                        label={name}
                        checked={on}
                        disabled={full}
                        disabledReason={full ? t("mcp.form.sourcesFull", { max: MAX_KNOWLEDGE_SOURCES }) : undefined}
                        onChange={(event) =>
                          set({
                            knowledgeSources: event.target.checked
                              ? [...form.knowledgeSources, name]
                              : form.knowledgeSources.filter((source) => source !== name),
                          })
                        }
                      />
                    </li>
                  );
                })}
              </ul>
            ) : null}
            {keptSources.length > 0 ? (
              <p className="text-caption text-fg-muted">{t("mcp.form.keptSources", { sources: keptSources.join(", ") })}</p>
            ) : null}
            {shownError("knowledgeSources") ? <InlineError>{shownError("knowledgeSources")?.[0]}</InlineError> : null}
          </fieldset>

          <div className="grid gap-4 md:grid-cols-2">
            <Field id="mcp-audience" label={t("mcp.field.audience")} required help={t("mcp.help.audience")} errors={shownError("audience")}>
              <Select id="mcp-audience" value={form.audience} onChange={(event) => set({ audience: event.target.value as Audience })}>
                {AUDIENCES.map((audience) => (
                  <option key={audience} value={audience} disabled={breadth(audience) > breadth(widest)}>
                    {t(`mcp.audience.${audience}`)}
                  </option>
                ))}
              </Select>
            </Field>
            {form.audience === "project-list" ? (
              <Field
                id="mcp-projects"
                label={t("mcp.field.allowedProjects")}
                help={t("mcp.help.allowedProjects")}
                errors={refusedProjects ? [refusedProjects] : shownError("allowedProjects")}
              >
                <Combobox
                  id="mcp-projects"
                  labelled
                  label={t("mcp.field.allowedProjects")}
                  multiple
                  value={form.allowedProjects}
                  onChange={(allowedProjects) => set({ allowedProjects })}
                  options={(projects.data ?? []).map((name) => ({ value: name, label: name, group: t("mcp.form.yourProjects") }))}
                  loading={projects.isPending}
                  failed={projects.isError ? { reason: projects.error.message, retry: () => void projects.refetch() } : undefined}
                  invalid={Boolean(refusedProjects)}
                  empty={t("mcp.form.noProjects")}
                />
              </Field>
            ) : null}
          </div>
          {form.audience === "public" ? (
            <Alert tone="warning" role="status">
              {t("mcp.form.publicLane")}
            </Alert>
          ) : null}

          <ToolPreview members={picked} />

          {proposal.error && !refusedProjects ? (
            <Alert tone="danger" role="alert">
              {proposal.error}
            </Alert>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}
