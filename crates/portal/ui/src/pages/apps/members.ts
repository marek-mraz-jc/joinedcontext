// What an App's roles and members are made of, apart from the section that edits them: the
// access pages and People import these helpers, and importing them from RolesAndMembers.tsx
// pulled that whole section into the bundle every page loads first (T-4883, 519 of 518 KB gz).

/** Who holds a role: one person by e-mail or one group of the organization (AP-91). */
export interface Subject {
  user?: string;
  group?: string;
}

export interface AppRole {
  name: string;
  title?: string | Record<string, string>;
  description?: string;
}

interface AppAccess {
  role: string;
  subjects: Subject[];
}

/** The part of an App's spec this section reads and writes; the rest travels on untouched. */
export interface RolesSpec {
  roles?: AppRole[];
  access?: AppAccess[];
  [rest: string]: unknown;
}

const same = (a: Subject, b: Subject) => (a.user ?? null) === (b.user ?? null) && (a.group ?? null) === (b.group ?? null);

/** The members of `role`, in the order the manifest lists them. */
export function membersOf(spec: RolesSpec, role: string): Subject[] {
  return (spec.access ?? []).filter((entry) => entry.role === role).flatMap((entry) => entry.subjects);
}

/** The spec with `subject` added to `role`, or `null` when it already holds it. */
export function withMember(spec: RolesSpec, role: string, subject: Subject): RolesSpec | null {
  if (membersOf(spec, role).some((held) => same(held, subject))) return null;
  const access = spec.access ?? [];
  const listed = access.some((entry) => entry.role === role);
  return {
    ...spec,
    access: listed
      ? access.map((entry) => (entry.role === role ? { ...entry, subjects: [...entry.subjects, subject] } : entry))
      : [...access, { role, subjects: [subject] }],
  };
}

/**
 * The spec with `subject` taken out of `role`. A role left with nobody loses its `access` entry,
 * since jc-core refuses an entry without subjects (AP-91); the role itself stays declared.
 */
export function withoutMember(spec: RolesSpec, role: string, subject: Subject): RolesSpec {
  return {
    ...spec,
    access: (spec.access ?? [])
      .map((entry) =>
        entry.role === role ? { ...entry, subjects: entry.subjects.filter((held) => !same(held, subject)) } : entry,
      )
      .filter((entry) => entry.subjects.length > 0),
  };
}

/** The default group of `role` of App `app`, which the App's Change commits with the role (AP-118). */
export function defaultGroupOf(app: string, role: string): string {
  return `${app}-${role}`;
}

/** Who else holds `role`: every subject of its access entries except its default group (AP-119). */
export function othersOf(spec: RolesSpec, role: string, group: string): Subject[] {
  return membersOf(spec, role).filter((subject) => subject.group !== group);
}

/** A person is named by the e-mail the realm signs them in with, lower case (AP-91). */
export function asUser(typed: string): string | null {
  const email = typed.trim().toLowerCase();
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email) ? email : null;
}

/**
 * The e-mails of a pasted list, split on commas, semicolons, spaces and line breaks, each checked
 * as `asUser` does and named once; `invalid` keeps every word that is not an e-mail, as typed.
 */
export function emailsIn(text: string): { emails: string[]; invalid: string[] } {
  const emails: string[] = [];
  const invalid: string[] = [];
  for (const word of text.split(/[\s,;]+/).filter(Boolean)) {
    const email = asUser(word);
    if (email === null) invalid.push(word);
    else if (!emails.includes(email)) emails.push(email);
  }
  return { emails, invalid };
}
