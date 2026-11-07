// The connector door under load (T-5978, AG-143..AG-147): every call to /c/{id} verifies the
// token, reads the connector, computes the owner's live rights narrowed by its scope, answers and
// writes an audit row. This script holds a mix of tools/list, jc_resource_list and
// jc_resource_get over 980 connectors of 49 people and judges p95 per tool against the budget.
// In the middle of the run it removes one person's binding (the harness's control listener) and
// the very next call of that person's connector must be refused, while their connector for
// another project still answers: nothing that decides a call survives a rights change.
//
// Run by the Portal test the_connector_door_holds_its_budget_under_load, which writes the plan
// (door URL, connectors and their tokens) to JC_LOAD_PLAN:
//   cargo test --release --test connector_scenarios -- --ignored the_connector_door_holds_its_budget_under_load
// JC_K6_RATE (calls a second, default 60; each person may make 120 a minute, so 49 people hold at
// most 98), JC_K6_DURATION (default 60s), JC_K6_P95_MS (the budget, default 50).
import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';
import exec from 'k6/execution';

const plan = JSON.parse(open(__ENV.JC_LOAD_PLAN));
const RATE = Number(__ENV.JC_K6_RATE || 60);
const DURATION = __ENV.JC_K6_DURATION || '60s';
const P95_MS = Number(__ENV.JC_K6_P95_MS || 50);
const TOOLS = ['tools/list', 'jc_resource_list', 'jc_resource_get'];
const PEOPLE = 49;
const proven = new Counter('revoke_proven');

const budget = {};
for (const tool of TOOLS) budget[`http_req_duration{tool:${tool}}`] = [`p(95)<${P95_MS}`];

export const options = {
  scenarios: {
    mix: {
      executor: 'constant-arrival-rate',
      exec: 'mix',
      rate: RATE,
      timeUnit: '1s',
      duration: DURATION,
      // Started ahead: a VU started during a stall counts as a dropped arrival.
      preAllocatedVUs: 200,
      maxVUs: 200,
    },
    revoke: { executor: 'per-vu-iterations', exec: 'revoke', vus: 1, iterations: 1, startTime: '20s' },
  },
  thresholds: {
    ...budget,
    'checks{scenario:mix}': ['rate==1'],
    'checks{scenario:revoke}': ['rate==1'],
    revoke_proven: ['count==1'],
    // A door that cannot keep up drops arrivals instead of answering slowly: that fails too.
    dropped_iterations: ['count==0'],
  },
  summaryTrendStats: ['avg', 'p(50)', 'p(95)', 'p(99)', 'max'],
};

function call(connector, tool) {
  const message =
    tool === 'tools/list'
      ? { jsonrpc: '2.0', id: 1, method: 'tools/list', params: { project: connector.project } }
      : {
          jsonrpc: '2.0',
          id: 1,
          method: 'tools/call',
          params: {
            name: tool,
            arguments: {
              project: connector.project,
              kind: 'Endpoint',
              ...(tool === 'jc_resource_get' ? { name: `${connector.project}-counters` } : {}),
            },
          },
        };
  return http.post(`${plan.door}/c/${connector.id}`, JSON.stringify(message), {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${connector.token}` },
    tags: { tool, name: `/c/{id} ${tool}` },
  });
}

function answered(res, connector, tool) {
  if (res.status !== 200) return false;
  const body = res.json();
  if (body.error || (body.result && body.result.isError)) return false;
  if (tool === 'tools/list') return (body.result.tools || []).length > 0;
  return res.body.includes(`${connector.project}-counters`);
}

function refused(res, connector) {
  if (res.status === 403 || res.status === 404) return true;
  if (res.status !== 200) return false;
  const body = res.json();
  return Boolean(body.error || body.result.isError) && !res.body.includes(`${connector.project}-counters`);
}

export function mix() {
  const n = exec.scenario.iterationInTest;
  // Round the people, then each person's connectors: the plan lists them person by person, and
  // walking it in that order sends one person 20 calls in a row, so a stall of 150 ms puts more
  // than that person's 8 parallel calls in flight and the door rightly answers 429.
  const perPerson = plan.connectors.length / PEOPLE;
  const connector = plan.connectors[(n % PEOPLE) * perPerson + (Math.floor(n / PEOPLE) % perPerson)];
  const tool = TOOLS[n % TOOLS.length];
  const res = call(connector, tool);
  check(
    res,
    {
      'not limited': (r) => r.status !== 429,
      answered: (r) => answered(r, connector, tool),
    },
    { tool },
  );
}

export function revoke() {
  const { revoked, other } = plan;
  const before = check(call(revoked, 'jc_resource_list'), {
    'the connector reads before the unbind': (r) => answered(r, revoked, 'jc_resource_list'),
  });
  const unbound = check(http.post(plan.control, null, { tags: { name: 'control' } }), {
    'the binding is removed': (r) => r.status === 204,
  });
  const next = check(call(revoked, 'jc_resource_list'), {
    'the next call is refused': (r) => refused(r, revoked),
  });
  const kept = check(call(other, 'jc_resource_list'), {
    'the same person reads their other project': (r) => answered(r, other, 'jc_resource_list'),
  });
  if (before && unbound && next && kept) proven.add(1);
}

export function handleSummary(data) {
  const lines = [`connector door, ${RATE} calls/s for ${DURATION}, budget p95 < ${P95_MS} ms`];
  for (const tool of TOOLS) {
    const m = data.metrics[`http_req_duration{tool:${tool}}`];
    if (!m) continue;
    const v = m.values;
    const ok = m.thresholds && Object.values(m.thresholds).every((t) => t.ok);
    lines.push(
      `  ${tool.padEnd(17)} p50 ${v['p(50)'].toFixed(1)} ms  p95 ${v['p(95)'].toFixed(1)} ms  ` +
        `p99 ${v['p(99)'].toFixed(1)} ms  max ${v.max.toFixed(1)} ms  ${ok ? 'ok' : 'OVER BUDGET'}`,
    );
  }
  for (const c of data.root_group.checks || []) {
    if (c.fails > 0) lines.push(`  check "${c.name}": ${c.fails} of ${c.passes + c.fails} failed`);
  }
  const revoke = data.metrics.revoke_proven;
  lines.push(`  revoke: ${revoke && revoke.values.count === 1 ? 'next call refused' : 'NOT PROVEN'}`);
  const out = { stdout: `${lines.join('\n')}\n` };
  if (__ENV.JC_LOAD_SUMMARY) out[__ENV.JC_LOAD_SUMMARY] = JSON.stringify(data);
  return out;
}
