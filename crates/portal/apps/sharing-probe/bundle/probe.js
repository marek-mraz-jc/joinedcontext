// The sharing probe (T-4911): what the static host wrote into `#jc-config` (SDK-02) and how many
// entities the App's own endpoint answers to this reader, read with a same-origin `fetch` so the
// edge's session for this App's host rides along and nothing else does (AP-133).

/** The count request of the primary endpoint and its first type, or null without an endpoint. */
export function countUrl(config) {
  const endpoint = config?.endpoints?.[0];
  const slug = endpoint?.slug ?? config?.slug;
  if (!slug) return null;
  const params = new URLSearchParams({ limit: "1", count: "true" });
  const type = endpoint?.types?.[0];
  if (type) params.set("type", type);
  return `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?${params}`;
}

/** The total of an NGSI-LD count answer (the `NGSILD-Results-Count` header), or null. */
export function totalOf(headers) {
  const raw = headers.get("NGSILD-Results-Count");
  return raw !== null && /^\d+$/.test(raw) ? Number(raw) : null;
}

if (typeof document !== "undefined") {
  const $ = (id) => document.getElementById(id);
  let config = {};
  try {
    config = JSON.parse($("jc-config")?.textContent ?? "{}");
  } catch {
    config = {};
  }
  $("app").textContent = config.appName ?? location.hostname.split(".")[0];
  $("person").textContent = config.user?.name ?? config.user?.email ?? "Nobody (anonymous visitor)";

  const out = $("count");
  const url = countUrl(config);
  if (!url) {
    out.textContent = "This application reads no endpoint.";
    out.dataset.state = "no-endpoint";
  } else {
    fetch(url, { headers: { Accept: "application/ld+json" }, credentials: "same-origin" })
      .then((response) => {
        const total = totalOf(response.headers);
        if (response.ok && total !== null) {
          out.textContent = `${total} entities`;
          out.dataset.entityCount = String(total);
          out.dataset.state = "counted";
        } else {
          out.textContent = `The endpoint refused this reader (HTTP ${response.status}).`;
          out.dataset.state = "refused";
          out.dataset.httpStatus = String(response.status);
        }
      })
      .catch(() => {
        out.textContent = "The endpoint did not answer.";
        out.dataset.state = "unreachable";
      });
  }
}
