// The history write bench (T-5243, run.sh beside it): PATCH level (no observedAt, half repeat the stored value) and speed (observedAt) on N entities.
import http from 'k6/http';
import { check } from 'k6';
const B = __ENV.BROKER_URL, N = Number(__ENV.N || 200);
const H = { headers: { 'Content-Type': 'application/json' } };
export const options = { vus: Number(__ENV.VUS || 16), duration: __ENV.DUR || '60s', summaryTrendStats: ['avg', 'p(95)'] };
export function setup() {
  if (__ENV.CREATE !== '1') return;
  for (let i = 0; i < N; i++) {
    const r = http.post(`${B}/ngsi-ld/v1/entities`, JSON.stringify({ id: `urn:ngsi-ld:Tank:b${i}`, type: 'Tank',
      level: { type: 'Property', value: 0 }, speed: { type: 'Property', value: 0, observedAt: new Date().toISOString() } }), H);
    if (r.status !== 201) throw new Error(`create ${i}: ${r.status} ${r.body}`);
  }
}
export default function () {
  const i = Math.floor(Math.random() * N);
  const level = __ITER % 2 === 0 ? 7 : __ITER;
  const r = http.patch(`${B}/ngsi-ld/v1/entities/urn:ngsi-ld:Tank:b${i}/attrs`, JSON.stringify({
    level: { type: 'Property', value: level },
    speed: { type: 'Property', value: __ITER % 50, observedAt: new Date().toISOString() } }), H);
  check(r, { '204': (x) => x.status === 204 });
}
