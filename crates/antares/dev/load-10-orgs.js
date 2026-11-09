// SPDX-License-Identifier: Apache-2.0
// Ten organizations at once (T-6628): k6 side of dev/load-10-orgs.sh. Each Tenant t1..tN is one
// organization with its own caller (x-jc-client load-tN) and rule document; the broker's
// joinedcontext engine and forced RLS decide every request.
//
//   MODE=seed  every Tenant gets SENSORS Sensors, BUILDINGS Buildings and VEHICLES Vehicles, the
//              Sensors' temperature written HISTORY times with observedAt (the only Attribute
//              with history, by the Tenant's retention rule).
//   MODE=load  constant arrival of RATE req/s over all Tenants (RATE/TENANTS each), WARMUP then
//              DURATION: 70 % query by type + q, 15 % entity by id, 10 % temporal lastN, 5 % upsert.
//              XTENANT_EVERY-th query instead reads another Tenant with the own caller.
//
// Isolation: every id a read answers must carry the reading Tenant's prefix (urn:...:tN-...);
// one that does not counts in `leaks`. A cross-Tenant read must answer 403 or 404; a 2xx counts
// in `leaks` too, any other status in `xtenant_unexpected`. Both have the threshold count == 0,
// so a leak fails the run (exit 99). A dropped connection (status 0) counts in status_0 only.
import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';

const BROKER = __ENV.BROKER_URL || 'http://127.0.0.1:29080';
const MODE = __ENV.MODE || 'load';
const TENANTS = Number(__ENV.TENANTS || 10);
const RATE = Number(__ENV.RATE || 10000);
const DURATION = __ENV.DURATION || '60s';
const WARMUP = __ENV.WARMUP || '15s';
const SENSORS = Number(__ENV.SENSORS || 4000);
const BUILDINGS = Number(__ENV.BUILDINGS || 3000);
const VEHICLES = Number(__ENV.VEHICLES || 3000);
const HISTORY = Number(__ENV.HISTORY || 3);
const XTENANT_EVERY = Number(__ENV.XTENANT_EVERY || 100);
const MAX_VUS = Number(__ENV.MAX_VUS_PER_TENANT || 300);
const SUMMARY = __ENV.SUMMARY || 'summary.json';

const TYPES = [
  ['Sensor', SENSORS],
  ['Building', BUILDINGS],
  ['Vehicle', VEHICLES],
];
const KINDS = ['query', 'get', 'temporal', 'upsert'];
const STATUSES = [200, 201, 204, 207, 400, 401, 403, 404, 409, 413, 429, 500, 502, 503, 504];

const leaks = new Counter('leaks');
const xtenantUnexpected = new Counter('xtenant_unexpected');
const byStatus = {};
for (const s of STATUSES.concat(['0', 'other'])) byStatus[s] = new Counter(`status_${s}`);

const tenant = (i) => `t${i}`;
const pad = (n) => String(n).padStart(5, '0');
const urn = (t, type, n) => `urn:ngsi-ld:${type}:${t}-${pad(n)}`;
const ownPrefix = (t) => new RegExp(`^urn:ngsi-ld:[A-Za-z]+:${t}-\\d{5}$`);

function headers(t, caller, extra) {
  return Object.assign(
    { 'NGSILD-Tenant': t, 'x-jc-client': `load-${caller}`, Accept: 'application/json' },
    extra || {},
  );
}

function entity(t, type, n, observedAt) {
  const e = {
    id: urn(t, type, n),
    type,
    level: { type: 'Property', value: n % 100 },
    status: { type: 'Property', value: n % 7 === 0 ? 'maintenance' : 'ok' },
    location: {
      type: 'GeoProperty',
      value: { type: 'Point', coordinates: [17 + (n % 1000) / 1000, 48 + (n % 997) / 1000] },
    },
  };
  if (type === 'Sensor') {
    e.temperature = { type: 'Property', value: 10 + (n % 25), observedAt, unitCode: 'CEL' };
  } else if (type === 'Vehicle') {
    e.speed = { type: 'Property', value: n % 120 };
  } else {
    e.name = { type: 'Property', value: `Building ${n} of ${t}` };
  }
  return e;
}

function post(t, body) {
  const r = http.post(`${BROKER}/ngsi-ld/v1/entityOperations/upsert?options=update`, JSON.stringify(body), {
    headers: headers(t, t, { 'Content-Type': 'application/json' }),
    tags: { kind: 'seed' },
  });
  if (r.status !== 201 && r.status !== 204) {
    throw new Error(`seed ${t}: ${r.status} ${String(r.body).slice(0, 300)}`);
  }
}

function seed() {
  const t = tenant(__VU);
  const hour = 3600 * 1000;
  for (let round = 0; round < HISTORY; round++) {
    const at = new Date(Date.now() - (HISTORY - round) * hour).toISOString();
    for (const [type, count] of TYPES) {
      if (round > 0 && type !== 'Sensor') continue;
      for (let from = 0; from < count; from += 500) {
        const batch = [];
        for (let n = from; n < Math.min(count, from + 500); n++) batch.push(entity(t, type, n, at));
        post(t, batch);
      }
    }
  }
}

function count(r) {
  (byStatus[r.status] || byStatus.other).add(1);
}

function idsOf(r) {
  let body;
  try {
    body = r.json();
  } catch (_) {
    return [];
  }
  const list = Array.isArray(body) ? body : [body];
  return list.filter((e) => e && typeof e === 'object' && 'id' in e).map((e) => e.id);
}

function checkOwn(r, t) {
  if (r.status < 200 || r.status > 299) return;
  const own = ownPrefix(t);
  const foreign = idsOf(r).filter((id) => !own.test(id)).length;
  if (foreign > 0) leaks.add(foreign, { tenant: t });
}

const pick = (n) => Math.floor(Math.random() * n);

function query(t, iter) {
  const [type] = TYPES[pick(3)];
  if (XTENANT_EVERY > 0 && iter % XTENANT_EVERY === XTENANT_EVERY - 1) {
    const other = tenant(((Number(t.slice(1)) + pick(TENANTS - 1)) % TENANTS) + 1);
    const r = http.get(`${BROKER}/ngsi-ld/v1/entities?type=${type}&limit=5`, {
      headers: headers(other, t),
      tags: { kind: 'xtenant' },
      responseCallback: http.expectedStatuses(403, 404),
    });
    count(r);
    if (r.status >= 200 && r.status <= 299 && idsOf(r).length > 0) leaks.add(1, { tenant: t, via: 'xtenant' });
    // status 0 is a dropped connection (counted in status_0), not an answer the engine gave
    else if (r.status !== 0 && r.status !== 403 && r.status !== 404) xtenantUnexpected.add(1, { tenant: t });
    check(r, { 'cross-tenant read refused': (x) => x.status === 403 || x.status === 404 });
    return;
  }
  const lo = pick(95);
  const q = encodeURIComponent(`level>=${lo};level<${lo + 5}`);
  const r = http.get(`${BROKER}/ngsi-ld/v1/entities?type=${type}&q=${q}&limit=20`, {
    headers: headers(t, t),
    tags: { kind: 'query' },
  });
  count(r);
  checkOwn(r, t);
  check(r, { 'query 200': (x) => x.status === 200 });
}

function get(t) {
  const [type, n] = TYPES[pick(3)];
  const r = http.get(`${BROKER}/ngsi-ld/v1/entities/${urn(t, type, pick(n))}`, {
    headers: headers(t, t),
    tags: { kind: 'get' },
  });
  count(r);
  checkOwn(r, t);
  check(r, { 'get 200': (x) => x.status === 200 });
}

function temporal(t) {
  const r = http.get(`${BROKER}/ngsi-ld/v1/temporal/entities/${urn(t, 'Sensor', pick(SENSORS))}?attrs=temperature&lastN=5`, {
    headers: headers(t, t),
    tags: { kind: 'temporal' },
  });
  count(r);
  checkOwn(r, t);
  check(r, { 'temporal 200': (x) => x.status === 200 });
}

function upsert(t) {
  const n = pick(SENSORS);
  const body = [
    {
      id: urn(t, 'Sensor', n),
      type: 'Sensor',
      temperature: { type: 'Property', value: 10 + pick(25), observedAt: new Date().toISOString(), unitCode: 'CEL' },
    },
  ];
  const r = http.post(`${BROKER}/ngsi-ld/v1/entityOperations/upsert?options=update`, JSON.stringify(body), {
    headers: headers(t, t, { 'Content-Type': 'application/json' }),
    tags: { kind: 'upsert' },
  });
  count(r);
  check(r, { 'upsert 2xx': (x) => x.status === 201 || x.status === 204 });
}

export function mixed() {
  const t = __ENV.T || 't1';
  const r = Math.random() * 100;
  if (r < 70) query(t, Math.floor(Math.random() * 1e9));
  else if (r < 85) get(t);
  else if (r < 95) temporal(t);
  else upsert(t);
}

function scenarios() {
  const out = {};
  const per = Math.round(RATE / TENANTS);
  for (let i = 1; i <= TENANTS; i++) {
    const base = {
      executor: 'constant-arrival-rate',
      rate: per,
      timeUnit: '1s',
      preAllocatedVUs: Math.min(MAX_VUS, Math.ceil(per / 5)),
      maxVUs: MAX_VUS,
      exec: 'mixed',
      env: { T: tenant(i) },
    };
    out[`warm_t${i}`] = Object.assign({}, base, { duration: WARMUP, tags: { tenant: tenant(i), phase: 'warm' } });
    out[`load_t${i}`] = Object.assign({}, base, { duration: DURATION, startTime: WARMUP, tags: { tenant: tenant(i), phase: 'load' } });
  }
  return out;
}

function thresholds() {
  // a threshold per sub-metric is what makes k6 keep it in the summary; `max>=0` always holds
  const th = { leaks: ['count==0'], xtenant_unexpected: ['count==0'], 'http_reqs{phase:load}': ['count>=0'] };
  for (const k of KINDS.concat(['xtenant'])) {
    th[`http_req_duration{phase:load,kind:${k}}`] = ['max>=0'];
    th[`http_req_failed{phase:load,kind:${k}}`] = ['rate>=0'];
  }
  for (let i = 1; i <= TENANTS; i++) {
    th[`http_req_duration{phase:load,tenant:${tenant(i)}}`] = ['max>=0'];
    th[`http_req_failed{phase:load,tenant:${tenant(i)}}`] = ['rate>=0'];
    for (const k of KINDS) th[`http_req_duration{phase:load,tenant:${tenant(i)},kind:${k}}`] = ['max>=0'];
  }
  return th;
}

export const options =
  MODE === 'seed'
    ? { scenarios: { seed: { executor: 'per-vu-iterations', vus: TENANTS, iterations: 1, maxDuration: '30m', exec: 'seedAll' } } }
    : {
        scenarios: scenarios(),
        thresholds: thresholds(),
        summaryTrendStats: ['avg', 'min', 'med', 'p(95)', 'p(99)', 'max'],
        discardResponseBodies: false,
        noConnectionReuse: false,
        systemTags: ['status', 'method', 'scenario', 'expected_response'],
      };

export function seedAll() {
  seed();
}

export function handleSummary(data) {
  const out = { stdout: `summary written to ${SUMMARY}\n` };
  out[SUMMARY] = JSON.stringify(data, null, 1);
  return out;
}
