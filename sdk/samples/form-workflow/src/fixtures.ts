import type { AccessDocument, JcUser, Row, Schema } from "@joinedcontext/sdk";

/** The person signed in; their requests are the rows below. */
export const USER: JcUser = { id: "5b0e7c1a-2f4d-4c8e-9a61-3d2b7f0c9e14", name: "Aino Virtanen" };

/** The endpoint's JSON Schema, as Model Tools generates it from `model.linkml.yaml`. */
export const SCHEMA: Schema = {
  ServiceRequest: {
    required: ["category", "title", "description", "address", "submittedBy", "dateSubmitted", "status"],
    properties: {
      category: { type: "string", enum: ["pothole", "streetlight", "graffiti", "litter", "other"] },
      title: { type: "string", pattern: ".{5,80}" },
      description: { type: "string", pattern: "[\\s\\S]{20,2000}" },
      address: { type: "string", pattern: ".{3,120}" },
      district: { type: ["string", "null"], enum: ["Kallio", "Kamppi", "Vallila", "Töölö", "Pasila"] },
      contactEmail: { type: ["string", "null"], pattern: "[^@\\s]+@[^@\\s]+\\.[^@\\s]+" },
      mayContact: { type: ["boolean", "null"] },
      submittedBy: { type: "string" },
      dateSubmitted: { type: "string", format: "date-time" },
      status: { type: "string", enum: ["received", "inProgress", "done"] },
    },
  },
};

const request = (local: string, title: string, category: string, status: string, day: number): Row => ({
  id: `urn:ngsi-ld:ServiceRequest:hel.fi:requests:${local}`,
  type: "ServiceRequest",
  title,
  category,
  status,
  description: `${title}, reported through the app.`,
  address: "Fleminginkatu 12",
  district: "Kallio",
  submittedBy: USER.id,
  dateSubmitted: `2026-09-${String(day).padStart(2, "0")}T08:15:00Z`,
});

/** The person's earlier requests, one in each state. */
export const ROWS: Row[] = [
  request("r1", "Streetlight out on Fleminginkatu", "streetlight", "done", 3),
  request("r2", "Deep pothole at the tram stop", "pothole", "inProgress", 17),
  request("r3", "Overflowing bin in Karhupuisto", "litter", "received", 23),
];

/** A resident may read requests and create one; nothing else. */
export const RESIDENT_ACCESS: AccessDocument = {
  permissions: [{ resource: { type: "ServiceRequest" }, actions: ["queryEntity", "retrieveEntity", "createEntity"], attributes: "*" }],
  prohibitions: [],
};
