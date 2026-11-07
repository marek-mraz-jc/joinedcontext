import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// The preview bridge of a run page (T-0679, SDK-18, AP-63). `vite preview` serves the built
// bundle with no Portal behind it, so the API and the endpoint are answered from the browser;
// the run page, its sandboxed frame and the messages between them are the real ones. The
// preview document stands in for the SDK: a form whose Save and Delete buttons send the SDK's
// `jc-request` and print the `jc-response`, and which reports one error on load.

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const ID = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002";
const ENTITY = `/api/endpoint/${SLUG}/ngsi-ld/v1/entities/${encodeURIComponent(ID)}`;
const PREVIEW = "/api/v1/projects/helsinki/agent-runs/run-1/preview?v=3";

const RUN = {
  id: "run-1",
  project: "helsinki",
  appName: "bikes",
  endpointName: "bikes",
  endpointSlug: SLUG,
  appClass: "static",
  visibility: "internal",
  prompt: "A form to close a docking station.",
  status: "awaiting_approval",
  steps: 3,
  tokensUsed: 1200,
  previewUrl: PREVIEW,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T10:00:00Z",
  dataNeeds: [{ types: ["BikeHireDockingStation"], attrs: ["status"], operations: ["queryEntity", "retrieveEntity", "updateAttrs"] }],
};

const DOCUMENT = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>bikes</title></head>
<body>
<button id="save">Save</button>
<button id="delete">Delete</button>
<output id="answer" aria-label="answer"></output>
<script>
  let seq = 0;
  const request = (method, path, body) => new Promise((resolve) => {
    const id = ++seq;
    addEventListener("message", function onMessage(event) {
      if (event.source !== parent || event.data?.kind !== "jc-response" || event.data.id !== id) return;
      removeEventListener("message", onMessage);
      resolve(event.data);
    });
    parent.postMessage({ kind: "jc-request", id, method, path, body }, "*");
  });
  const show = (answer) => {
    document.getElementById("answer").textContent = answer.status + " " + (answer.body?.detail ?? "");
  };
  document.getElementById("save").onclick = () =>
    request("PATCH", ${JSON.stringify(`${ENTITY}/attrs`)}, { status: { type: "Property", value: "closed" } }).then(show);
  document.getElementById("delete").onclick = () => request("DELETE", ${JSON.stringify(ENTITY)}).then(show);
  parent.postMessage({ kind: "jc-error", message: "Cannot read properties of undefined (reading 'value')", file: "src/pages/Overview.tsx", line: 42 }, "*");
</script>
</body></html>`;

interface Seen {
  endpoint: { method: string; path: string; csrf?: string; body: unknown }[];
  errors: unknown[];
}

async function stub(page: Page): Promise<Seen> {
  const seen: Seen = { endpoint: [], errors: [] };
  await page.route("**/api/endpoint/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    seen.endpoint.push({
      method: request.method(),
      path: url.pathname,
      csrf: request.headers()["x-csrf-token"],
      body: request.postDataJSON() as unknown,
    });
    await route.fulfill({ status: 204 });
  });
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-approver"] });
    }
    if (url.pathname === "/api/v1/projects") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    }
    if (url.pathname === "/api/v1/projects/helsinki/agent-runs") {
      return json({ items: [RUN] });
    }
    if (url.pathname === "/api/v1/projects/helsinki/agent-runs/run-1") {
      return json(RUN);
    }
    if (url.pathname.endsWith("/agent-runs/run-1/events")) {
      return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    }
    if (url.pathname.endsWith("/agent-runs/run-1/preview")) {
      return route.fulfill({ status: 200, contentType: "text/html", body: DOCUMENT });
    }
    if (url.pathname.endsWith("/agent-runs/run-1/preview-errors")) {
      seen.errors.push(route.request().postDataJSON());
      return route.fulfill({ status: 204 });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return seen;
}

test("the preview writes through the run page, only what the data needs grant", async ({ page }) => {
  const seen = await stub(page);
  await page.goto("/projects/helsinki/apps/bikes?lang=en");
  // A `__Host-` cookie is Secure: CDP refuses one for an http URL, the page on 127.0.0.1 (a
  // secure context) may write it, as the Portal's Set-Cookie does over https (T-4395).
  await page.evaluate(() => {
    document.cookie = "__Host-jc_csrf=csrf-e2e; Secure; Path=/";
  });
  const frame = page.frameLocator('iframe[title="Preview of Bikes"]');

  await frame.getByRole("button", { name: "Save" }).click();
  await expect(frame.getByLabel("answer")).toHaveText("204");
  expect(seen.endpoint).toEqual([
    { method: "PATCH", path: `${ENTITY}/attrs`, csrf: "csrf-e2e", body: { status: { type: "Property", value: "closed" } } },
  ]);

  // Deleting is not in the run's data needs: refused at once, and nothing reaches the endpoint.
  await frame.getByRole("button", { name: "Delete" }).click();
  await expect(frame.getByLabel("answer")).toHaveText("403 The run's data needs do not name deleteEntity.");
  expect(seen.endpoint).toHaveLength(1);

  await expect.poll(() => seen.errors).toEqual([
    { message: "Cannot read properties of undefined (reading 'value')", file: "src/pages/Overview.tsx", line: 42 },
  ]);
});
