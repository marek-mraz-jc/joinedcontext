import { retainSearchParams } from "@tanstack/react-router";

/**
 * Inside a copy every in-app link stays in it (T-4868): a link that does not name `workspace`
 * carries the current one. Without this the pipeline page's Edit opened the editor outside the
 * copy and its Propose went to the project, while the person still thought they were in the copy.
 */
export const keepCopy = retainSearchParams<{ workspace?: string }>(["workspace"]);


/** A DNS-1123 label, the shape of an Organization's name (PF-121). */
export const ORG_NAME = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

/** The Organization the switcher chose stays chosen on every in-app link (PF-124). */
export const keepOrganization = retainSearchParams<{ org?: string }>(["org"]);
