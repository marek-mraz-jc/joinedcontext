import type { AccessDocument } from "@joinedcontext/sdk";
import { WRITABLE } from "../alerts";

const READ = { resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const };

/** What the endpoint answers a viewer: the read item of `app.yaml` and nothing else. */
export const VIEWER: AccessDocument = { permissions: [READ], prohibitions: [] };

/** What it answers a steward: the read item and the two items granted to `steward`. */
export const STEWARD: AccessDocument = {
  permissions: [
    READ,
    { resource: { type: "Alert" }, actions: ["createEntity", "updateAttrs"], attributes: WRITABLE },
    { resource: { type: "Alert" }, actions: ["deleteEntity"], attributes: "*" },
  ],
  prohibitions: [],
};
