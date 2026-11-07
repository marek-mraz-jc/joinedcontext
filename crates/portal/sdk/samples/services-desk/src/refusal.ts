import { ProblemError, ServiceRefusedError } from "@joinedcontext/sdk";

const SERVICE: Record<ServiceRefusedError["service"], string> = {
  files: "Files",
  email: "Email",
  jobs: "Scheduled jobs",
  ai: "The model",
};
const LAYER: Record<NonNullable<ServiceRefusedError["layer"]>, string> = {
  organization: "organization",
  project: "project",
  app: "App",
};

/**
 * What a refusal says to a person: which layer switched the service off, which quota is used up
 * and until when, or which role an action needs. Never a bare status code.
 */
export function refusal(err: unknown): string {
  if (err instanceof ServiceRefusedError) {
    const service = SERVICE[err.service];
    if (err.layer) return `${service} is switched off for this ${LAYER[err.layer]}.`;
    if (err.quota) return `${service}: the ${err.quota} quota is used up${err.resetAt ? ` until ${err.resetAt}` : ""}.`;
  }
  if (err instanceof ProblemError) return err.detail ?? err.title;
  return err instanceof Error ? err.message : String(err);
}
