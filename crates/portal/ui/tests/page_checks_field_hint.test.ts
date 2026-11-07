import { afterEach, describe, expect, it } from "vitest";
import { pageFindings } from "./pageChecks";

// T-3852: the live walk flagged 522 ticked boxes on the endpoint page by their outerHTML. A box
// wrapped in its <label> is labelled, and a box of a list is described by its group.
describe("the walkers' field-hint check", () => {
  const findings = () => pageFindings({ namespaces: [], layout: false }).filter((finding) => finding.startsWith("field-hint"));
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("takes a box's group description as its hint", () => {
    document.body.innerHTML = `<main><form><fieldset aria-describedby="help"><legend>Types</legend>
      <p id="help">Tick the types a caller reads here.</p>
      <label><input type="checkbox"> Vehicle</label></fieldset></form></main>`;
    expect(findings()).toEqual([]);
  });

  it("names an undescribed box by the label that wraps it", () => {
    document.body.innerHTML = `<main><form><fieldset><legend>Types</legend>
      <label><input type="checkbox"> Vehicle</label></fieldset></form></main>`;
    expect(findings()).toEqual(["field-hint: Vehicle"]);
  });

  it("does not lend a group's description to a text field in it", () => {
    document.body.innerHTML = `<main><form><fieldset aria-describedby="help"><legend>Filter</legend>
      <p id="help">What the filter keeps.</p>
      <label>Area <input type="text"></label></fieldset></form></main>`;
    expect(findings()).toEqual(["field-hint: Area"]);
  });
});

// T-3851: a grid's id column shows the id it opens; a URN is bare where a label belongs.
describe("the walkers' bare-urn check", () => {
  const findings = () => pageFindings({ namespaces: [], layout: false }).filter((finding) => finding.startsWith("bare-urn"));
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("lets a cell show the id it opens and flags one as a heading", () => {
    document.body.innerHTML = `<main><h1>Helsinki city context</h1><p>What this space holds.</p>
      <table><tbody><tr><td><button>urn:ngsi-ld:Event:1</button></td></tr></tbody></table>
      <h2>urn:ngsi-ld:Event:2</h2></main>`;
    expect(findings()).toEqual(["bare-urn: urn:ngsi-ld:Event:2"]);
  });
});
