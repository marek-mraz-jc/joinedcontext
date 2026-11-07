// node --test scripts/cost-reconcile.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { infrastructure, reconcile } from "./cost-reconcile.mjs";

const CSV = `month,project,part,item,bytes,cost,currency
2026-09,doprava,node,,,10.00,EUR
2026-09,doprava,storage,cars,1073741824,0.50,EUR
2026-09,doprava,ai,"help, public",,4.20,EUR
2026-09,,platform-node,,,30.00,EUR
2026-09,,platform-storage,,,4.50,EUR
`;

test("the infrastructure is every line but AI", () => {
  assert.deepEqual(infrastructure(CSV), { total: 45, currency: "EUR" });
});

test("within 10 % of the invoice reconciles, past it does not", () => {
  assert.equal(reconcile(45, 48).ok, true);
  assert.equal(reconcile(45, 51).ok, false);
  assert.throws(() => reconcile(45, 0), /positive/);
});

test("an export of another shape or mixed currencies is refused", () => {
  assert.throws(() => infrastructure("a,b\n1,2"), /header/);
  assert.throws(() => infrastructure(CSV + "2026-09,,platform-node,,,1.00,USD\n"), /currencies/);
});
