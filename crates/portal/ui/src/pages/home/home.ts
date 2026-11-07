/**
 * What the project home page decides (T-3233, T-3235): the first-run steps from the project's own
 * state, the person's role from their grants, and the cards worth showing that role. Pure, so
 * each rule is tested without a page.
 */
import type { Effective } from "../../api/permissions";

export type StepKey = "upload" | "share" | "publish";

export interface Step {
  key: StepKey;
  done: boolean;
  /** Where the step is taken. */
  to: string;
}

/** The project's state as the steps read it. */
export interface ProjectState {
  /** The project's Endpoints, by name. */
  endpoints: string[];
  /** How many of them carry a catalogue publication (`spec.publish.ckan`). */
  published: number;
  /** The first space that holds entities, if any was counted. */
  spaceWithData?: string;
}

/**
 * The first dataset in about ten minutes (T-5109), each step ticked by what the project holds,
 * never by a click: data in a space (a CSV upload, its model suggested from the columns, which
 * also proposes the space's Endpoint), that Endpoint, private to the organization until the
 * person chooses otherwise, and its publication as a public dataset with its catalogue entry.
 */
export function firstRunSteps(project: string, state: ProjectState): Step[] {
  const base = `/projects/${project}`;
  const endpoint = state.endpoints[0];
  return [
    { key: "upload", done: state.spaceWithData !== undefined, to: `${base}/spaces/upload` },
    { key: "share", done: endpoint !== undefined, to: `${base}/endpoints/new` },
    {
      key: "publish",
      done: state.published > 0,
      to: endpoint ? `${base}/endpoints/${encodeURIComponent(endpoint)}` : `${base}/endpoints`,
    },
  ];
}

export type Role = "steward" | "editor" | "viewer";

/**
 * Who the person is in this project, by what their grants let them do: approving makes a
 * steward, proposing an editor, reading alone a viewer. Undefined until the document arrives.
 */
export function roleOf(effective: Effective | undefined): Role | undefined {
  if (!effective || !Array.isArray(effective.grants)) {
    return undefined;
  }
  if (effective.bootstrap === true) {
    return "steward";
  }
  const verbs = new Set(
    effective.grants.flatMap((grant) => ((grant.rule as { verbs?: string[] }).verbs ?? [])),
  );
  if (verbs.has("approve")) return "steward";
  if (verbs.has("propose")) return "editor";
  return "viewer";
}

export type CardKey = "approvals" | "failing" | "invites" | "drafts" | "data" | "dashboards";

export interface Card {
  key: CardKey;
  count: number;
  /** Where one click takes the person. */
  to: string;
}

/** What the home page knows; a count it could not read is undefined and its card is not shown. */
export interface Facts {
  approvalsWaiting?: number;
  failingPipelines?: number;
  invitesPending?: number;
  drafts?: { count: number; to: string };
  spaceWithData?: string;
  dashboards?: string[];
}

/** The cards the role acts on, in the order it would, and none that would show zero. */
export function cardsFor(project: string, role: Role, facts: Facts): Card[] {
  const base = `/projects/${project}`;
  const all: Record<Role, (Card | undefined)[]> = {
    steward: [
      facts.approvalsWaiting ? { key: "approvals", count: facts.approvalsWaiting, to: `${base}/approvals` } : undefined,
      facts.failingPipelines ? { key: "failing", count: facts.failingPipelines, to: `${base}/pipelines` } : undefined,
      facts.invitesPending ? { key: "invites", count: facts.invitesPending, to: "/organization/people" } : undefined,
    ],
    editor: [
      facts.drafts?.count ? { key: "drafts", count: facts.drafts.count, to: facts.drafts.to } : undefined,
      facts.failingPipelines ? { key: "failing", count: facts.failingPipelines, to: `${base}/pipelines` } : undefined,
    ],
    viewer: [
      facts.spaceWithData ? { key: "data", count: 1, to: `${base}/spaces/${facts.spaceWithData}` } : undefined,
      facts.dashboards?.length
        ? { key: "dashboards", count: facts.dashboards.length, to: `${base}/dashboards/${facts.dashboards[0]}` }
        : undefined,
    ],
  };
  return all[role].filter((card): card is Card => card !== undefined);
}

/** A draft's page, where `?draft=` resumes it (AG-61); undefined for a kind with no such page. */
export function draftPage(project: string, kind: string, name: string): string | undefined {
  const plural: Record<string, string> = {
    ContextSpace: "spaces",
    Pipeline: "pipelines",
    DataSource: "datasources",
    Dashboard: "dashboards",
    Endpoint: "endpoints",
    Policy: "policies",
  };
  const where = plural[kind];
  return where ? `/projects/${project}/${where}?draft=${encodeURIComponent(name)}` : undefined;
}

/**
 * The sample project the first-run checklist offers in `project` (PF-109): the first one the
 * person may read other than the project they are in, or none.
 */
export function sampleToTry(project: string, samples: readonly string[]): string | undefined {
  return samples.find((sample) => sample !== project);
}

/**
 * Where a newcomer's first step for their role is taken (PF-108): a viewer opens the data, an
 * editor connects a source, a steward reviews what waits for them.
 */
export function welcomeStep(project: string, role: Role): string {
  const base = `/projects/${project}`;
  return { viewer: `${base}/spaces`, editor: `${base}/datasources/new`, steward: `${base}/approvals` }[role];
}
