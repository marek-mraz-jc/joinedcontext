/*
 * What a picture cannot prove at 400 px (T-7512): text under 12 px, a tap target under 44x44 px, a
 * page wider than the screen, contrast under WCAG AA (4.5:1, large text 3:1). Evaluated in each
 * frame by phoneCheck.cts and bins/jc/smoke/ui/phone_check.py, with axe already injected; `width` is
 * the screen's for the page and null for a frame, which is held to its own box.
 *
 * ponytail: text drawn on a canvas, a CSS transform's scale and a pseudo-element's larger hit area
 * are not seen; measure those by hand in the spec that draws them.
 */
(async (width) => {
  const hits = [];
  const readable = /[\p{L}\p{N}]/u;
  const add = (kind, element, detail, words = "") => {
    const name = element.tagName.toLowerCase() + (element.id ? `#${element.id}` : "");
    const label = (words || element.getAttribute("aria-label") || element.textContent || "").replace(/\s+/g, " ").trim().slice(0, 40);
    hits.push({ kind, what: `${name}${label ? ` "${label}"` : ""}`.slice(0, 120), detail });
  };
  const shown = (element) => {
    if (!element.checkVisibility({ opacityProperty: true, visibilityProperty: true })) return false;
    for (let box = element; box && box !== document.body; box = box.parentElement) {
      const style = getComputedStyle(box);
      const { width: w, height: h } = box.getBoundingClientRect();
      // Words kept for a screen reader: a box of one pixel, or one clipped away.
      if (style.overflow !== "visible" && style.display !== "contents" && (w <= 1 || h <= 1)) return false;
      if (style.clipPath === "inset(50%)") return false;
    }
    return true;
  };
  const onPage = (rect) => rect.width > 0 && rect.height > 0 && rect.right + scrollX > 0 && rect.bottom + scrollY > 0;
  // Under another box, such as the page behind a modal dialog: the topmost element at its centre is
  // neither it, inside it nor around it, even once scrolled to (and back): a box below the fold or
  // scrolled out of its list is measured as a person scrolling would see it. One that cannot reach
  // the screen is measured too.
  // ponytail: elementFromPoint passes over pointer-events: none, so such a box over words hides nothing here.
  const atCentre = (rect) => {
    const x = rect.left + rect.width / 2;
    const y = rect.top + rect.height / 2;
    return x < 0 || y < 0 || x >= innerWidth || y >= innerHeight ? null : document.elementFromPoint(x, y);
  };
  const shows = (element, top) => element.contains(top) || top.contains(element);
  const covered = (element, measure) => {
    const now = atCentre(measure());
    if (now && shows(element, now)) return false;
    const kept = [];
    for (let box = element; box; box = box.parentElement) kept.push([box, box.scrollLeft, box.scrollTop]);
    element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    const top = atCentre(measure());
    for (const [box, left, up] of kept) box.scrollTo({ left, top: up, behavior: "instant" });
    return Boolean(top) && !shows(element, top);
  };

  const text = new Map();
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walk.nextNode(); node; node = walk.nextNode()) {
    const parent = node.parentElement;
    if (!parent || parent.closest("script, style, noscript, template, option") || !readable.test(node.data) || !shown(parent)) continue;
    const range = document.createRange();
    range.selectNodeContents(node);
    const first = () => [...range.getClientRects()].find(onPage);
    if (first() && !covered(parent, first)) text.set(parent, node.data);
  }
  for (const field of document.querySelectorAll("input:not([type=hidden], [type=checkbox], [type=radio], [type=range], [type=color]), select, textarea")) {
    const words = field.tagName === "SELECT" ? field.selectedOptions[0]?.text : field.value || field.placeholder;
    // A password's value is never copied into a report.
    const box = () => field.getBoundingClientRect();
    if (words && shown(field) && onPage(box()) && !covered(field, box)) text.set(field, field.type === "password" ? "(password)" : words);
  }
  for (const [element, words] of text) {
    let size = parseFloat(getComputedStyle(element).fontSize);
    const matrix = element instanceof SVGGraphicsElement ? element.getScreenCTM() : null;
    if (matrix) size *= Math.sqrt(Math.abs(matrix.a * matrix.d - matrix.b * matrix.c));
    // font-size 0 is how an icon hides its glyph.
    if (size > 0 && size < 11.95) add("font", element, `${size.toFixed(1)} px, under 12`, words);
  }

  const controls = "button, a[href], input:not([type=hidden]), select, textarea, summary, [role=button], [role=link], [role=tab], [role=checkbox], [role=radio], [role=switch], [role=menuitem], [role=option], [role=slider], [role=combobox]";
  for (const control of document.querySelectorAll(controls)) {
    const boxes = [control, ...(control.labels ?? [])]
      .filter(shown)
      .filter((part) => onPage(part.getBoundingClientRect()) && !covered(part, () => part.getBoundingClientRect()))
      .map((part) => part.getBoundingClientRect());
    if (!boxes.length) continue;
    // A link inside a sentence is as tall as its line (WCAG 2.5.8's inline exception).
    if (control.matches("a[href]") && getComputedStyle(control).display === "inline") {
      const around = [...control.parentNode.childNodes].filter((node) => node.nodeType === Node.TEXT_NODE);
      if (around.some((node) => readable.test(node.data))) continue;
    }
    const w = Math.max(...boxes.map((box) => box.width));
    const h = Math.max(...boxes.map((box) => box.height));
    if (w < 43.5 || h < 43.5) add("target", control, `${Math.round(w)}x${Math.round(h)} px, under 44x44`);
  }

  const root = document.documentElement;
  const room = width ?? root.clientWidth;
  // A phone widens its layout viewport to what overflows, so the page is held to the screen's width.
  const wide = width ? Math.max(root.scrollWidth, innerWidth) : root.scrollWidth;
  if (wide > room + 1) {
    const cause = [...document.body.querySelectorAll("*")].find((element) => shown(element) && element.getBoundingClientRect().right + scrollX > room + 1);
    add("overflow", cause ?? document.body, `the page is ${wide} px wide in ${room}`);
  }

  const found = await window.axe.run(document, { runOnly: { type: "rule", values: ["color-contrast"] }, iframes: false });
  for (const node of found.violations.flatMap((violation) => violation.nodes)) {
    const measured = node.any[0]?.data ?? {};
    hits.push({ kind: "contrast", what: node.target.join(" ").slice(0, 120), detail: `${measured.contrastRatio}:1, under ${measured.expectedContrastRatio ?? "4.5:1"}` });
  }
  return hits;
})
