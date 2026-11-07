/** T-2738: the model as an ER diagram, one line per relationship with its multiplicities (DM-13, DM-64, DM-65, UI-84). */
import { useState } from "react";
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LinkmlEditor } from "../src/pages/models/LinkmlEditor";
import { LinkmlGraphView } from "../src/pages/models/LinkmlGraphView";
import { graphData, parseModel } from "../src/pages/models/linkml";
import { expectNoRawKeys } from "./checks";
import { expectNoAxeViolations, inEveryLocale, renderPart } from "./page_contract";

/** Rozvoj's schools: every cardinality once, the source of each marked by its delete rule. */
const SCHOOLS = `id: https://rozvoj.sk/models/schools
name: schools
prefixes:
  rz: https://rozvoj.sk/terms/
default_prefix: rz
classes:
  School:
    slots: [principal, pupils]
  User:
    slots: [principalOf, school, courses]
  Course:
    slots: [students, grades]
  Grade:
    slots: [course]
slots:
  principal:
    range: User
    required: true
    inverse: principalOf
    annotations: { ngsi_ld_kind: Relationship, on_delete: restrict }
  principalOf:
    range: School
    inverse: principal
    annotations: { ngsi_ld_kind: Relationship }
  pupils:
    range: User
    multivalued: true
    inverse: school
    annotations: { ngsi_ld_kind: Relationship, on_delete: cascade }
  school:
    range: School
    required: true
    inverse: pupils
    annotations: { ngsi_ld_kind: Relationship }
  course:
    range: Course
    required: true
    inverse: grades
    annotations: { ngsi_ld_kind: Relationship, on_delete: cascade }
  grades:
    range: Grade
    multivalued: true
    inverse: course
    annotations: { ngsi_ld_kind: Relationship }
  students:
    range: User
    multivalued: true
    inverse: courses
    annotations: { ngsi_ld_kind: Relationship, on_delete: set-null }
  courses:
    range: Course
    multivalued: true
    inverse: students
    annotations: { ngsi_ld_kind: Relationship }
`;

const opening = (name: string, inverse: string, source: string, target: string) =>
  en.models.graph.openRelationship
    .replace("{name}", name)
    .replace("{inverse}", inverse)
    .replace("{source}", source)
    .replace("{target}", target);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("the diagram's lines", () => {
  it("draws each pair as one line with the multiplicity at both ends", () => {
    const { edges } = graphData(parseModel(SCHOOLS));
    // No `range` line for an end: the pair is the one line.
    expect(edges.filter((edge) => edge.kind === "range")).toEqual([]);
    expect(
      edges.map((edge) => [edge.from, edge.label, edge.fromMultiplicity, edge.cardinality, edge.toMultiplicity, edge.inverse, edge.to]),
    ).toEqual([
      // One School has one principal, and a User is principal of at most one School.
      ["School", "principal", "0..1", "one-to-one", "1", "principalOf", "User"],
      // One School has many pupils; each pupil has exactly one School.
      ["School", "pupils", "1", "one-to-many", "*", "school", "User"],
      ["Course", "students", "*", "many-to-many", "*", "courses", "User"],
      // Many Grades share one Course, and a Grade has exactly one.
      ["Grade", "course", "*", "many-to-one", "1", "grades", "Course"],
    ]);
  });

  it("shows the multiplicities on the drawn line, and the field with its inverse", () => {
    renderPart(<LinkmlGraphView source={SCHOOLS} />);
    const line = screen.getByRole("button", { name: opening("pupils", "school", "School", "User") });
    const texts = [...line.querySelectorAll("text")].map((text) => text.textContent);
    expect(texts).toEqual(["pupils / school", "1", "*"]);
    expect(screen.getByText(en.models.graph.legendRelationship)).toBeInTheDocument();
  });

  it("draws a class related to itself as a loop", () => {
    const self = `name: org
classes:
  Unit:
    slots: [parent, children]
slots:
  parent:
    range: Unit
    inverse: children
    annotations: { ngsi_ld_kind: Relationship, on_delete: restrict }
  children:
    range: Unit
    multivalued: true
    inverse: parent
    annotations: { ngsi_ld_kind: Relationship }
`;
    renderPart(<LinkmlGraphView source={self} />);
    const loop = screen.getByRole("button", { name: opening("parent", "children", "Unit", "Unit") });
    expect(loop.querySelector("path")?.getAttribute("d")).toContain("C");
  });
});

describe("what a press opens", () => {
  it("opens the class by its box and the relationship by its line, by pointer and by keyboard", async () => {
    const user = userEvent.setup();
    const onOpenClass = vi.fn();
    const onOpenRelationship = vi.fn();
    renderPart(<LinkmlGraphView source={SCHOOLS} onOpenClass={onOpenClass} onOpenRelationship={onOpenRelationship} />);

    await user.click(screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "Grade") }));
    expect(onOpenClass).toHaveBeenCalledExactlyOnceWith("Grade");

    await user.click(screen.getByRole("button", { name: opening("course", "grades", "Grade", "Course") }));
    expect(onOpenRelationship).toHaveBeenLastCalledWith("Grade");

    screen.getByRole("button", { name: opening("students", "courses", "Course", "User") }).focus();
    await user.keyboard("{Enter}");
    expect(onOpenRelationship).toHaveBeenLastCalledWith("Course");
    await user.keyboard(" ");
    expect(onOpenRelationship).toHaveBeenCalledTimes(3);
  });

  it("offers Add relationship on the model's own classes only, and edits nothing itself", async () => {
    const user = userEvent.setup();
    const onAddRelationship = vi.fn();
    const people = `name: people
classes:
  Person:
    slots: []
`;
    const importing = `name: x
imports: [people]
classes:
  School:
    slots: []
`;
    renderPart(<LinkmlGraphView source={importing} imports={{ people }} onAddRelationship={onAddRelationship} />);
    expect(screen.queryByRole("button", { name: en.models.graph.addRelationship.replace("{name}", "Person") })).toBeNull();

    screen.getByRole("button", { name: en.models.graph.addRelationship.replace("{name}", "School") }).focus();
    await user.keyboard("{Enter}");
    expect(onAddRelationship).toHaveBeenCalledExactlyOnceWith("School");
  });

  // T-3589: the structure view's form, opened on the drawing itself in a dialog.
  it("opens the Add relationship form from a box, on the drawing", async () => {
    const user = userEvent.setup();
    function Editor() {
      const [source, setSource] = useState(SCHOOLS);
      return <LinkmlEditor source={source} onChange={setSource} initialView="graph" />;
    }
    renderPart(<Editor />);
    await user.click(screen.getByRole("button", { name: en.models.graph.addRelationship.replace("{name}", "Course") }));
    const dialog = screen.getByRole("dialog", { name: "New relationship from Course" });
    expect(within(dialog).getByRole("form", { name: "Add a relationship from Course" })).toBeInTheDocument();
  });
});

describe("reading it without the picture", () => {
  it("lists every relationship in a table below the drawing", () => {
    renderPart(<LinkmlGraphView source={SCHOOLS} />);
    const table = screen.getByRole("table", { name: en.models.graph.table });
    // Below the drawing, which keeps the whole width (T-6146).
    expect(screen.getByRole("group", { name: en.models.graph.title }).compareDocumentPosition(table) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const rows = within(table).getAllByRole("row").slice(1).map((row) => row.textContent);
    expect(rows).toEqual([
      "One School has one UserSchool.principalUser.principalOfSchool 0..1 — 1 User",
      "One School has many User entitiesSchool.pupilsUser.schoolSchool 1 — * User",
      "Many Course entities have many User entitiesCourse.studentsUser.coursesCourse * — * User",
      "Many Grade entities share one CourseGrade.courseCourse.gradesGrade * — 1 Course",
    ]);
  });

  it("zooms in, out and back to fit, and says the level", async () => {
    const user = userEvent.setup();
    const { container } = renderPart(<LinkmlGraphView source={SCHOOLS} />);
    const svg = container.querySelector("svg[role='group']") as SVGSVGElement;
    const width = () => Number(svg.getAttribute("width"));
    const natural = width();

    await user.click(screen.getByRole("button", { name: en.models.graph.zoomIn }));
    expect(width()).toBeCloseTo(natural * 1.25);
    expect(screen.getByText("Zoom 125 %")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.models.graph.zoomOut }));
    await user.click(screen.getByRole("button", { name: en.models.graph.zoomOut }));
    expect(width()).toBeCloseTo(natural / 1.25);
    // The frame has no width in a DOM without layout, so fit falls back to the natural size.
    await user.click(screen.getByRole("button", { name: en.models.graph.fit }));
    expect(width()).toBeCloseTo(natural);
  });

  // T-6090 review: the fit stops at 75 %, below which a slot line is too small to read; a
  // drawing wider than that scrolls in its frame.
  it("fits a drawing wider than its frame to the frame's width on load, never below readable, and never enlarges one (T-4991)", () => {
    const wide = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    const widthAt = (frame: number) => {
      Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => frame });
      const view = renderPart(<LinkmlGraphView source={SCHOOLS} />);
      const svg = view.container.querySelector("svg[role='group']") as SVGSVGElement;
      const width = Number(svg.getAttribute("width"));
      const level = view.getByText(/^Zoom \d+ %$/).textContent;
      view.unmount();
      return { width, level };
    };
    try {
      const natural = widthAt(100000);
      expect(natural.level).toBe("Zoom 100 %");
      // A frame a little narrower than the drawing: it fits the frame's width.
      const frame = Math.round(natural.width * 0.9);
      expect(widthAt(frame).width).toBeCloseTo(frame - 16, 0);
      // A frame just under the 60 % floor: the page's diagram shrinks to the frame's width, down to
      // 50 %, rather than cut a box (T-6254 judge, model-catalog-400).
      const snug = Math.round((natural.width * 0.55) + 16);
      expect(widthAt(snug).width).toBeCloseTo(snug - 16, 0);
      // A frame far narrower: the drawing stays at 60 %, the overview floor (owner 2026-10-31), and scrolls.
      const narrow = widthAt(216);
      expect(narrow.level).toBe("Zoom 60 %");
      expect(narrow.width).toBeCloseTo(natural.width * 0.6, 0);
    } finally {
      if (wide) Object.defineProperty(HTMLElement.prototype, "clientWidth", wide);
    }
  });

  // T-6254 judge: the page's overview cut its bottom row by the frame's own padding and sat in the
  // frame's left third. It now fits the whole drawing inside the padding, centred.
  it("opens fitted to the frame's height inside its padding, centred", () => {
    const wide = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    const high = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
    try {
      Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 100000 });
      const natural = renderPart(<LinkmlGraphView source={SCHOOLS} />);
      const naturalHeight = Number(natural.container.querySelector("svg[role='group']")?.getAttribute("height"));
      natural.unmount();
      // A frame 80 % as high as the drawing, above the 60 % floor: the drawing takes the frame's inner height.
      const frame = Math.round(naturalHeight * 0.8) + 16;
      Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => frame });
      const view = renderPart(<LinkmlGraphView source={SCHOOLS} />);
      const svg = view.container.querySelector("svg[role='group']") as SVGSVGElement;
      expect(Number(svg.getAttribute("height"))).toBeCloseTo(frame - 16, 0);
      expect(svg).toHaveClass("mx-auto", "block");
      view.unmount();
    } finally {
      if (wide) Object.defineProperty(HTMLElement.prototype, "clientWidth", wide);
      if (high) Object.defineProperty(HTMLElement.prototype, "clientHeight", high);
    }
  });

  // T-6090 review: at the floor a phone frame opened on the empty corner beside a cut box; it now
  // opens scrolled to the first class. A drawing that fits is not scrolled.
  it("opens a drawing wider than its frame on its first class", () => {
    const wide = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    const scrolled = vi.fn();
    HTMLElement.prototype.scrollTo = scrolled as unknown as typeof HTMLElement.prototype.scrollTo;
    try {
      Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 100000 });
      renderPart(<LinkmlGraphView source={SCHOOLS} />).unmount();
      expect(scrolled).not.toHaveBeenCalled();
      Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 216 });
      renderPart(<LinkmlGraphView source={SCHOOLS} />).unmount();
      expect(scrolled).toHaveBeenCalledTimes(1);
      expect(scrolled.mock.calls[0]?.[0]).toEqual({ left: expect.any(Number), top: expect.any(Number) });
    } finally {
      if (wide) Object.defineProperty(HTMLElement.prototype, "clientWidth", wide);
      delete (HTMLElement.prototype as { scrollTo?: unknown }).scrollTo;
    }
  });

  it("draws names that arrive as markup as text", () => {
    const hostile = SCHOOLS.replaceAll("pupils", '"<img src=x onerror=alert(1)>"');
    const { container } = renderPart(<LinkmlGraphView source={hostile} />);
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByRole("button", { name: opening("<img src=x onerror=alert(1)>", "school", "School", "User") })).toBeInTheDocument();
  });

  it("reads in every locale with no raw key and no axe violation", async () => {
    await inEveryLocale(async () => {
      const { container, unmount } = renderPart(<LinkmlGraphView source={SCHOOLS} onAddRelationship={() => undefined} />);
      expectNoRawKeys(container);
      await expectNoAxeViolations(container);
      unmount();
    });
  });
});

/** T-2881: a city's schools with a key, typed fields and every kind of reference once. */
const CITY = `name: city
default_range: string
classes:
  Thing:
    slots: [id]
  School:
    is_a: Thing
    slots: [name, capacity, location, district, headteacher, pupils, tags]
  Pupil:
    slots: [school, clubs]
  Club:
    slots: [members]
  Headteacher:
    slots: [runs]
  District:
    slots: [label]
slots:
  id: { identifier: true, range: uriorcurie }
  name: { required: true }
  capacity: { range: integer }
  location:
    range: GeoJSONGeometry
    annotations: { ngsi_ld_kind: GeoProperty }
  tags: { multivalued: true }
  label: {}
  district: { range: District, required: true }
  headteacher:
    range: Headteacher
    required: true
    inverse: runs
    annotations: { ngsi_ld_kind: Relationship, on_delete: restrict }
  runs:
    range: School
    inverse: headteacher
    annotations: { ngsi_ld_kind: Relationship }
  pupils:
    range: Pupil
    multivalued: true
    inverse: school
    annotations: { ngsi_ld_kind: Relationship, on_delete: cascade }
  school:
    range: School
    required: true
    inverse: pupils
    annotations: { ngsi_ld_kind: Relationship }
  clubs:
    range: Club
    multivalued: true
    inverse: members
    annotations: { ngsi_ld_kind: Relationship, on_delete: set-null }
  members:
    range: Pupil
    multivalued: true
    inverse: clubs
    annotations: { ngsi_ld_kind: Relationship }
`;

describe("the diagram at first glance (T-2881)", () => {
  const rowsOf = (name: string) => graphData(parseModel(CITY)).nodes.find((node) => node.name === name)?.rows;

  it("types every field, puts the key first and marks every reference with how many it holds", () => {
    expect(rowsOf("School")).toEqual([
      // Inherited along `is_a`: a School is identified by the key Thing declares.
      { name: "id", type: "uriorcurie", key: "pk" },
      { name: "name", type: "string", required: true },
      { name: "capacity", type: "integer" },
      { name: "location", type: "GeoProperty" },
      { name: "district", type: "District", key: "fk", multiplicity: "1", required: true },
      { name: "headteacher", type: "Headteacher", key: "fk", multiplicity: "1", required: true },
      { name: "pupils", type: "Pupil", key: "fk", multiplicity: "*", multivalued: true },
      { name: "tags", type: "string", multivalued: true },
    ]);
    // No identifier anywhere up the chain: every NGSI-LD entity still has its `id`.
    expect(rowsOf("District")?.[0]).toEqual({ name: "id", type: "URN", key: "pk", implied: true });
  });

  it("draws the types, the PK and FK tags and the target of each reference inside the box", () => {
    renderPart(<LinkmlGraphView source={CITY} />);
    const school = screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "School") });
    const rows = [...school.querySelectorAll("g")].map((row) =>
      [...row.querySelectorAll("text")].map((text) => text.textContent).join(" "),
    );
    expect(rows).toEqual([
      "PK id uriorcurie",
      // A required field carries a `*` after its name (T-6094).
      "name * string",
      "capacity integer",
      "location GeoProperty",
      "FK district * → District 1",
      "FK headteacher * → Headteacher 1",
      "FK pupils → Pupil *",
      "tags string[]",
    ]);
    // The tag is told apart by its word and its border, never by colour alone.
    const tags = [...school.querySelectorAll("rect")].slice(1);
    expect(tags.map((tag) => tag.getAttribute("stroke-dasharray"))).toEqual([null, "2 1", "2 1", "2 1"]);
    expect(school.querySelector("g title")?.textContent).toBe(
      en.models.graph.row.pk.replace("{name}", "id").replace("{type}", "uriorcurie"),
    );
  });

  it("reads the cardinality at both ends of every line: 1..1, 1..N, N..M and a reference with no inverse", () => {
    renderPart(<LinkmlGraphView source={CITY} />);
    const ends = (name: string, inverse: string, source: string, target: string) =>
      [...screen.getByRole("button", { name: opening(name, inverse, source, target) }).querySelectorAll("text")]
        .slice(1)
        .map((text) => text.textContent);
    expect(ends("headteacher", "runs", "School", "Headteacher")).toEqual(["0..1", "1"]);
    expect(ends("pupils", "school", "School", "Pupil")).toEqual(["1", "*"]);
    expect(ends("clubs", "members", "Pupil", "Club")).toEqual(["*", "*"]);

    const { edges } = graphData(parseModel(CITY));
    const district = edges.find((edge) => edge.kind === "range" && edge.label === "district");
    // Required and single on School's side; nothing limits how many Schools share a District.
    expect([district?.fromMultiplicity, district?.toMultiplicity]).toEqual(["*", "1"]);
  });

  it("cuts a long name and keeps the whole row in its tooltip", () => {
    const long = CITY.replace("capacity: { range: integer }", "capacityOfTheWholeBuildingInPupils: { range: integer }")
      .replace("[name, capacity,", "[name, capacityOfTheWholeBuildingInPupils,");
    renderPart(<LinkmlGraphView source={long} />);
    const school = screen.getByRole("button", { name: en.models.graph.openClass.replace("{name}", "School") });
    // The short type leaves its room to the name (T-6094): it is cut at 27 characters, not 17.
    expect(school.textContent).toContain("capacityOfTheWholeBuilding…");
    expect([...school.querySelectorAll("title")].map((title) => title.textContent)).toContain(
      "capacityOfTheWholeBuildingInPupils: integer",
    );
  });

  it("has no axe violation with every marker drawn, in every locale", async () => {
    await inEveryLocale(async () => {
      const { container, unmount } = renderPart(<LinkmlGraphView source={CITY} />);
      expectNoRawKeys(container);
      await expectNoAxeViolations(container);
      unmount();
    });
  });
});
