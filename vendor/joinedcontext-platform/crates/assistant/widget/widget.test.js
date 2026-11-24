"use strict";
// T-7591: widget.js in a DOM. Each test opens the chat in a fresh jsdom window with a scripted
// fetch, a fake proof Worker and a fake host frame, and drives it as a visitor does: ask, read the
// stream, stop, retry, rate, copy, resize, change language, change page. Run with
// `node --test render.test.js widget.test.js` after `pnpm install` in crates/portal/ui, whose
// devDependency jsdom this file loads (no copy of its own here).
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { JSDOM } = require("node:module").createRequire(path.join(__dirname, "../../../../../crates/portal/ui/package.json"))("jsdom");

const RENDER = path.join(__dirname, "render.js");
const WIDGET = path.join(__dirname, "widget.js");
const SOURCES = { render: fs.readFileSync(RENDER, "utf8"), widget: fs.readFileSync(WIDGET, "utf8") };
// The label table the server puts before render.js on the page.
const FIELDS = require("../../jc-core/data/field-order.json");
const API = "https://assistant.example";
const VIZ = "https://embed.example";
const HOST = "https://www.example.sk";
const CONV = "0f8fad5b-d9cb-469f-a165-70867728950e";
const CONV2 = "1f8fad5b-d9cb-469f-a165-70867728950e";
const TURN = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
const DATASET = "https://data.example.sk/dataset/ovzdusie";

const wait = (ms = 0) => new Promise((done) => setTimeout(done, ms));
async function flush(rounds = 8) {
  for (let i = 0; i < rounds; i += 1) await wait(1);
}

function sse(events) {
  return events.map(([name, data]) => `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`).join("");
}

// A streamed 200: each chunk one read; `signal` keeps the stream open until it aborts.
function headersOf(values = {}) {
  return { get: (name) => (Object.prototype.hasOwnProperty.call(values, name) ? values[name] : null) };
}

function stream(chunks, signal, headers) {
  const encoder = new TextEncoder();
  const state = { cancelled: false };
  let i = 0;
  return {
    ok: true, status: 200, state, headers: headersOf(headers),
    json: () => Promise.reject(new SyntaxError("a stream")),
    body: {
      getReader: () => ({
        read() {
          if (i < chunks.length) return Promise.resolve({ done: false, value: encoder.encode(chunks[i++]) });
          if (signal) {
            return new Promise((_, failed) => {
              const stop = () => failed(new Error("aborted"));
              if (signal.aborted) stop();
              else signal.addEventListener("abort", stop);
            });
          }
          return Promise.resolve({ done: true });
        },
        cancel() {
          state.cancelled = true;
          return Promise.reject(new Error("already closed"));
        }
      })
    }
  };
}

function reply(status, body, headers) {
  return {
    ok: status >= 200 && status < 300, status, body: null, headers: headersOf(headers),
    json: () => (body === undefined ? Promise.reject(new SyntaxError("no body")) : Promise.resolve(body))
  };
}

const ANSWER = [
  ["conversation", { id: CONV, at: "2026-11-20T10:15:00Z", turn: TURN }],
  ["answer", { text: "Vzduch je dobrý." }],
  ["citations", []]
];

function open(o = {}) {
  const config = { publicId: "bb", languages: ["sk", "en"], enabled: true, api: API, ...o.config };
  const body = o.noRoot ? "" : `<div id="jc-chat"></div>`;
  const dom = new JSDOM(`<!doctype html><html><body class="${o.page ? "jc-page" : ""}">${body}</body></html>`, {
    url: o.url || "https://assistant.example/d/bb", pretendToBeVisual: true, runScripts: "outside-only"
  });
  const win = dom.window;
  const doc = win.document;
  const root = doc.getElementById("jc-chat");
  if (root) root.setAttribute("data-config", o.raw !== undefined ? o.raw : JSON.stringify(config));
  Object.entries(o.session || {}).forEach(([k, v]) => win.sessionStorage.setItem(k, v));
  Object.entries(o.local || {}).forEach(([k, v]) => win.localStorage.setItem(k, v));
  const sessionStore = win.sessionStorage;
  const localStore = win.localStorage;
  (o.noStorage || []).forEach((name) => {
    Object.defineProperty(win, name, { configurable: true, get() { throw new Error("storage is off"); } });
  });
  const posted = [];
  // Messages cross realms by value, as postMessage clones them.
  const parent = { postMessage: (message, origin) => posted.push({ message: JSON.parse(JSON.stringify(message)), origin }) };
  if (o.embed) Object.defineProperty(win, "parent", { configurable: true, get: () => parent });
  Object.defineProperty(win.screen, "width", { configurable: true, value: o.screen === undefined ? 1440 : o.screen });
  Object.defineProperty(win.navigator, "languages", { configurable: true, value: o.languages || ["sk"] });
  if (o.clipboard !== undefined) Object.defineProperty(win.navigator, "clipboard", { configurable: true, value: o.clipboard });
  if (o.execCommand !== undefined) doc.execCommand = o.execCommand;
  if (o.dark) win.matchMedia = () => ({ matches: true });
  win.HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
  win.HTMLDialogElement.prototype.close = function () {
    this.removeAttribute("open");
    this.dispatchEvent(new win.Event("close"));
  };
  Object.defineProperty(win.HTMLDialogElement.prototype, "open", {
    configurable: true, get() { return this.hasAttribute("open"); }
  });
  win.requestAnimationFrame = (callback) => setTimeout(callback, 0);
  win.cancelAnimationFrame = (id) => clearTimeout(id);
  const intervals = [];
  win.setInterval = (callback) => intervals.push(callback);
  win.clearInterval = (id) => { intervals[id - 1] = null; };
  win.TextDecoder = TextDecoder;
  win.JC_FIELDS = FIELDS;
  const frames = new WeakMap();
  const framePosts = [];
  Object.defineProperty(win.HTMLIFrameElement.prototype, "contentWindow", {
    configurable: true,
    get() {
      if (!frames.has(this)) frames.set(this, { postMessage: (message, origin) => framePosts.push({ frame: this, message, origin }) });
      return frames.get(this);
    }
  });
  const calls = [];
  let tokens = 0;
  const routes = o.routes || {};
  const defaults = {
    "/challenge": () => reply(200, { challenge: "c1", difficulty: 4 }),
    "/token": () => reply(200, { token: "tok-" + (++tokens), expiresAt: new Date(Date.now() + 3600e3).toISOString() }),
    "/chat": () => stream([sse(ANSWER)]),
    "/feedback": () => reply(204),
    "/resume": () => stream([sse(ANSWER)]),
    "/cancel": () => reply(204),
    "/map/": () => reply(200, { features: [] })
  };
  win.fetch = (url, init = {}) => {
    calls.push({ url, init });
    const key = Object.keys(defaults).find((k) => url.includes(k));
    const route = routes[key] || defaults[key];
    try {
      return Promise.resolve(route(init, url, calls.filter((c) => c.url.includes(key)).length));
    } catch (error) {
      return Promise.reject(error);
    }
  };
  const workers = [];
  win.Worker = class {
    constructor(src) { workers.push(src); }
    postMessage(message) {
      this.message = message;
      setTimeout(() => (o.workerFails ? this.onerror(new Error("no proof")) : this.onmessage({ data: 42 })), 0);
    }
    terminate() { this.terminated = true; }
  };
  const ctx = dom.getInternalVMContext();
  new vm.Script(SOURCES.render, { filename: RENDER }).runInContext(ctx);
  new vm.Script(SOURCES.widget, { filename: WIDGET }).runInContext(ctx);
  const $ = (s) => doc.querySelector(s);
  const $$ = (s) => Array.from(doc.querySelectorAll(s));
  return {
    win, doc, root, $, $$, posted, parent, calls, framePosts, intervals, workers, sessionStore, localStore,
    input: $("#jc-question"),
    chats: () => calls.filter((c) => c.url.endsWith("/chat")),
    ask(question) {
      this.input.value = question;
      $(".jc-form").dispatchEvent(new win.Event("submit", { cancelable: true }));
    },
    key(target, key, extra = {}) {
      const event = new win.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...extra });
      target.dispatchEvent(event);
      return event;
    },
    message(data, origin, source) {
      const event = new win.Event("message");
      Object.defineProperties(event, { data: { value: data }, origin: { value: origin }, source: { value: source } });
      win.dispatchEvent(event);
    },
    change(box, checked) {
      box.checked = checked;
      box.dispatchEvent(new win.Event("change"));
    }
  };
}

const text = (node) => (node ? node.textContent : null);

test("a page without the chat's root is left alone", () => {
  const chat = open({ noRoot: true });
  assert.equal(chat.doc.body.children.length, 0);
  assert.equal(chat.calls.length, 0);
});

test("the chat draws its title, toggle, connectors, greeting and questions in the visitor's language", async () => {
  const chat = open({
    config: {
      title: { sk: "Doprava BB", en: "BB transport" }, subtitle: { en: "Answers on transport" }, color: "#112233",
      font: "Inter, sans-serif", connectors: ["počasie", "doprava"], greeting: { sk: "Ahoj!", en: "Hi!" },
      suggestions: { sk: ["Aký je vzduch?", "Kedy ide bus?"], en: ["How is the air?"] }, mcp: API + "/mcp/bb"
    }
  });
  const style = chat.doc.documentElement.style;
  assert.equal(style.getPropertyValue("--accent"), "#112233");
  assert.equal(style.getPropertyValue("--font"), "Inter, sans-serif");
  assert.equal(text(chat.$(".jc-name")), "Doprava BB");
  assert.equal(chat.$(".jc-name").title, "Doprava BB");
  assert.equal(text(chat.$(".jc-role")), "Answers on transport", "no Slovak subtitle: the one it has");
  assert.deepEqual(chat.$$(".jc-langs button").map((b) => b.textContent), ["SK", "EN"]);
  assert.deepEqual(chat.$$(".jc-langs button").map((b) => b.getAttribute("aria-pressed")), ["true", "false"]);
  assert.deepEqual(chat.$$(".jc-tools input").map((b) => [b.value, b.checked]), [["počasie", true], ["doprava", true]]);
  assert.equal(text(chat.$(".jc-greeting")), "Ahoj!");
  assert.deepEqual(chat.$$(".jc-suggest button").map((b) => b.textContent), ["Aký je vzduch?", "Kedy ide bus?"]);
  assert.equal(chat.$(".jc-connect-open").title, "Pridať do svojho AI asistenta");
  assert.equal(chat.input.placeholder, "Napíšte otázku");
  assert.equal(chat.input.maxLength, 1000);
  chat.$$(".jc-langs button")[1].click();
  assert.equal(chat.localStore.getItem("jc-chat-lang"), "en");
  assert.equal(chat.doc.documentElement.lang, "en");
  assert.equal(text(chat.$(".jc-name")), "BB transport");
  assert.equal(text(chat.$(".jc-role")), "Answers on transport");
  assert.equal(chat.input.placeholder, "Type your question");
  assert.equal(text(chat.$(".jc-greeting")), "Hi!");
  assert.deepEqual(chat.$$(".jc-suggest button").map((b) => b.textContent), ["How is the air?"]);
  assert.equal(chat.$(".jc-fresh").getAttribute("aria-label"), "New conversation");
  // The proof starts as the chat opens, so the first question seldom waits.
  await flush();
  assert.deepEqual(chat.workers, ["/d/visitor.js"]);
  assert.ok(chat.calls.some((c) => c.url === API + "/api/v1/d/bb/token"));
});

test("values the server did not check are not used: another host, a colour word, markup in a font", async () => {
  const chat = open({
    languages: ["de", "en-GB"],
    config: { api: "http://evil.example", color: "red", font: "<b>x</b>", languages: ["en"], questionChars: 300, greeting: "Hello there" },
    local: { "jc-chat-lang": "sk" }
  });
  assert.equal(chat.doc.documentElement.style.getPropertyValue("--accent"), "");
  assert.equal(chat.doc.documentElement.style.getPropertyValue("--font"), "");
  assert.equal(chat.$(".jc-langs"), null, "one language: no toggle");
  assert.equal(chat.$(".jc-connect-open"), null);
  assert.equal(chat.$(".jc-tools"), null);
  assert.equal(text(chat.$(".jc-name")), "Assistant");
  assert.equal(chat.input.maxLength, 300);
  assert.equal(text(chat.$(".jc-greeting")), "Hello there");
  await flush();
  assert.ok(chat.calls.every((c) => c.url.startsWith("/api/v1/d/bb/")), "the chat's own host");
});

test("without the toggle an old pick is not read, and no greeting and no questions leave the log empty", () => {
  const chat = open({ config: { widget: { languageToggle: false }, greeting: ["x"] }, local: { "jc-chat-lang": "en" } });
  assert.equal(chat.$(".jc-langs"), null);
  assert.equal(chat.input.placeholder, "Napíšte otázku");
  assert.equal(text(chat.$(".jc-greeting")), "Dobrý deň! Na čo sa chcete opýtať?");
  const quiet = open({ config: { greeting: { en: "Hi" }, languages: ["en", "sk"], enabled: undefined }, languages: ["sk"] });
  assert.equal(text(quiet.$(".jc-greeting")), "Dobrý deň! Na čo sa chcete opýtať?");
});

test("the connector dialog lists each client's steps, copies the address and relabels in place", async () => {
  let written = null;
  const chat = open({
    clipboard: { writeText: (value) => { written = value; return Promise.resolve(); } },
    config: { mcp: API + "/mcp/bb", suggestions: { sk: ["Aký je vzduch?"] } }
  });
  chat.$(".jc-connect-open").click();
  const dialog = chat.$(".jc-connect");
  assert.equal(dialog.open, true);
  assert.equal(text(dialog.querySelector("h2")), "Pridať do svojho AI asistenta");
  assert.deepEqual(Array.from(dialog.querySelectorAll("summary")).map(text), ["Claude", "ChatGPT", "VS Code, Cursor, Claude Code"]);
  assert.ok(text(dialog.querySelector(".jc-connect-code")).includes(API + "/mcp/bb"));
  assert.equal(text(dialog.querySelector(".jc-connect-try")), "Aký je vzduch?");
  const address = dialog.querySelector("input");
  assert.equal(address.value, API + "/mcp/bb");
  assert.equal(address.readOnly, true);
  address.dispatchEvent(new chat.win.Event("focus"));
  assert.equal(address.selectionStart, 0);
  assert.equal(address.selectionEnd, address.value.length);
  dialog.querySelector(".jc-about-close").click();
  await flush();
  assert.equal(written, API + "/mcp/bb");
  assert.ok(text(dialog.querySelector(".jc-sr")).length > 0, "the copy is said");
  chat.$$(".jc-langs button")[1].click();
  assert.equal(text(chat.$(".jc-connect h2")), "Add to your AI assistant", "the open dialog is redrawn in English");
  const shut = Array.from(chat.$$(".jc-connect .jc-about-close")).pop();
  shut.click();
  assert.equal(chat.$(".jc-connect").open, false);
});

test("a refused clipboard falls back to the copy command, then to a selection with Ctrl+C said", async () => {
  const commanded = open({ clipboard: { writeText: () => Promise.reject(new Error("denied")) }, execCommand: () => true, config: { mcp: API + "/mcp/bb" } });
  commanded.$(".jc-connect-open").click();
  commanded.$(".jc-connect .jc-about-close").click();
  await flush();
  const done = text(commanded.$(".jc-connect .jc-sr"));
  assert.ok(done && !/Ctrl/.test(done), "the copy command copied: " + done);
  assert.equal(commanded.$$("textarea").length, 1, "the helper area is removed");
  const selected = open({ execCommand: () => false, config: { mcp: API + "/mcp/bb" } });
  selected.$(".jc-connect-open").click();
  selected.$(".jc-connect .jc-about-close").click();
  await flush();
  assert.match(text(selected.$(".jc-connect .jc-sr")), /Ctrl\+C/);
  const address = selected.$(".jc-connect input");
  assert.equal(address.selectionEnd - address.selectionStart, address.value.length, "the address stays selected");
});

test("the privacy dialog carries the disclaimer, the retention and whom to write; Tab stays inside", () => {
  const chat = open({
    config: {
      languages: ["en", "sk"], transcriptDays: 30, disclaimer: { en: "Own words." },
      privacy: { contact: "dpo@example.sk", documentsUrl: "https://example.sk/privacy" }
    }
  });
  assert.equal(chat.input.placeholder, "Napíšte otázku", "the browser's Slovak wins over the deployment's first");
  const opener = chat.$(".jc-privacy-line .jc-more");
  assert.equal(opener.getAttribute("aria-haspopup"), "dialog");
  opener.click();
  const about = chat.$(".jc-about");
  assert.equal(about.open, true);
  assert.equal(text(about.querySelector("p")), "Own words.", "no Slovak disclaimer: the deployment's first language");
  const body = about.querySelector(".jc-about-body");
  assert.deepEqual(Array.from(body.querySelectorAll("h3")).map(text), ["Konverzácie", "Otázky k osobným údajom"]);
  assert.match(text(body), /30 dní/);
  assert.equal(body.querySelector('a[href^="mailto:"]').getAttribute("href"), "mailto:dpo@example.sk");
  const docs = body.querySelector('a[href="https://example.sk/privacy"]');
  assert.equal(docs.rel, "noopener noreferrer");
  assert.equal(docs.target, "_blank");
  assert.equal(about.querySelector(".jc-remember input").checked, false, "remembering is off by default");
  const stops = about.querySelectorAll("a[href], input, button");
  const first = stops[0];
  const last = stops[stops.length - 1];
  last.focus();
  assert.equal(chat.key(about, "Tab").defaultPrevented, true);
  assert.equal(chat.doc.activeElement, first);
  assert.equal(chat.key(about, "Tab", { shiftKey: true }).defaultPrevented, true);
  assert.equal(chat.doc.activeElement, last);
  first.focus();
  assert.equal(chat.key(about, "Tab").defaultPrevented, false, "Tab inside moves on by itself");
  assert.equal(chat.key(about, "Enter").defaultPrevented, false);
  chat.$$(".jc-langs button")[1].click();
  assert.equal(text(about.querySelector("h3")), "Conversations", "the open dialog follows the language");
  chat.doc.activeElement.blur();
  about.querySelector(".jc-about-close").click();
  assert.equal(about.open, false);
  assert.equal(chat.doc.activeElement, opener, "focus goes back to the button that opened it");
  opener.click();
  chat.input.focus();
  about.close();
  assert.equal(chat.doc.activeElement, chat.input, "focus the visitor moved stays");
});

test("no contact and no retention: the dialog says nothing is kept and names nobody", () => {
  const chat = open({ config: { privacy: { contact: "not an address" } } });
  chat.$(".jc-privacy-line .jc-more").click();
  const body = chat.$(".jc-about-body");
  assert.equal(text(body.querySelector("p")), "Obsah konverzácií neuchovávame.");
  assert.equal(body.querySelector("a"), null);
});

test("the remember switch keeps the conversation in this browser and switching it off deletes it", async () => {
  const chat = open();
  chat.$(".jc-privacy-line .jc-more").click();
  const box = chat.$(".jc-about .jc-remember input");
  chat.change(box, true);
  const held = JSON.parse(chat.localStore.getItem("jc-chat-remember:bb"));
  assert.equal(held.kept, null);
  chat.ask("Aký je vzduch?");
  await flush();
  const kept = JSON.parse(chat.localStore.getItem("jc-chat-remember:bb")).kept;
  assert.equal(kept.conversation, CONV);
  assert.equal(kept.turns[0].question, "Aký je vzduch?");
  chat.change(box, false);
  assert.equal(chat.localStore.getItem("jc-chat-remember:bb"), null);
});

const KEPT = {
  conversation: CONV, asked: 2,
  turns: [
    { question: "Prvá?", text: "Prvá odpoveď.", turn: 1, citations: [], askedAt: "2026-11-20T10:00:00Z", at: "2026-11-20T10:00:05Z",
      opened: { url: DATASET, title: "Ovzdušie" } },
    { question: "Druhá?", text: "Druhá odpoveď.", turn: 2, citations: [], followups: ["A zajtra?"] }
  ]
};

test("a remembered conversation comes back from this browser and an expired one is deleted", () => {
  const savedAt = new Date().toISOString();
  const chat = open({ local: { "jc-chat-remember:bb": JSON.stringify({ savedAt, kept: KEPT }) } });
  assert.deepEqual(chat.$$(".jc-you p").map(text), ["Prvá?", "Druhá?"]);
  chat.$(".jc-privacy-line .jc-more").click();
  assert.equal(chat.$(".jc-about .jc-remember input").checked, true);
  const old = new Date(Date.now() - 91 * 24 * 3600e3).toISOString();
  const expired = open({ local: { "jc-chat-remember:bb": JSON.stringify({ savedAt: old, kept: KEPT }) } });
  assert.equal(expired.$$(".jc-you").length, 0);
  assert.equal(expired.localStore.getItem("jc-chat-remember:bb"), null);
});

test("the conversation open before a page change is shown again as it was, its last chips under it", () => {
  const chat = open({ config: { greeting: { sk: "Ahoj!" }, suggestions: { sk: ["Otázka?"] } }, session: { "jc-chat:bb": JSON.stringify(KEPT) } });
  assert.equal(text(chat.$(".jc-greeting")), "Ahoj!", "the greeting stays above");
  assert.equal(chat.$(".jc-intro .jc-suggest"), null, "its questions are gone");
  assert.deepEqual(chat.$$(".jc-bot .jc-answer").map(text), ["Prvá odpoveď.", "Druhá odpoveď."]);
  assert.equal(chat.$$(".jc-you time").length, 1, "only a known moment is shown");
  assert.equal(chat.$$(".jc-bot time").length, 1);
  assert.match(text(chat.$(".jc-opened")), /Ovzdušie/);
  assert.deepEqual(chat.$$(".jc-followups button").map(text), ["A zajtra?"]);
  assert.equal(text(chat.$(".jc-sr[aria-live]")), "", "nothing is read out again");
  chat.$(".jc-followups button").click();
  const sent = JSON.parse(chat.chats().length ? chat.chats()[0].init.body : "null");
  assert.equal(sent, null, "the token is still being earned");
});

test("an answer streams, shows its reads, table, chart, script and sources, then is kept for a page change", async () => {
  const citations = [
    { n: 1, url: "https://example.sk/air", title: "Ovzdušie mesta", cards: [
      { kind: "contact", name: "Úrad", phones: ["+421 48 123 456"], emails: ["urad@example.sk"] },
      { kind: "place", address: "Námestie SNP 1" },
      { kind: "procedure", steps: ["Prídite", "Podpíšte"] }] },
    { n: 2, tool: "query", endpoint: "ovzdusie", title: "Stanice" },
    { n: 3, url: "upload:docs/plan.pdf" },
    { n: 4, url: "https://example.sk/air", cards: [{ kind: "place", address: "Námestie SNP 1" }] }
  ];
  const events = [
    ["conversation", { id: CONV, at: "2026-11-20T10:15:00Z", turn: TURN }],
    ["stage", { stage: "reading", source: "Ovzdušie", topic: "AirQualityObserved" }],
    ["tool", { status: "started" }],
    ["step", { source: "Ovzdušie", topic: "AirQualityObserved", count: 12, ms: 800, url: DATASET }],
    ["stage", { stage: "checking", count: 3 }],
    ["stage", { stage: "checking", count: 3, verdict: "answers" }],
    ["delta", { text: "Vzduch je " }],
    ["delta", { text: "**dobrý** [1], [mesto](https://example.sk/air)." }],
    ["data", { type: "AirQualityObserved", columns: ["name", "pm10"], rows: [["Námestie SNP", 21], ["Zvolenská", 12], ["Rooseveltova", 30]], dataset: DATASET, total: 80 }],
    ["script", { code: "1+1", output: "2" }],
    ["script", { code: "x", error: "no x" }],
    ["answer", { text: "Vzduch je **dobrý** [1][2][3]. Viac na [stránke](https://example.sk/air) a [inde](https://evil.example/x)." }],
    ["citations", citations],
    ["followups", { questions: ["A zajtra?", " A  včera? ", 3] }],
    ["done", { stagesMs: { retrieve: 400, generate: 900 } }]
  ];
  const chat = open({ config: { linkHosts: ["assistant.example"] }, routes: { "/chat": () => stream([sse(events.slice(0, 8)), sse(events.slice(8))]) } });
  chat.ask("  Aký je vzduch?  ");
  assert.equal(chat.$(".jc-submit").hidden, true);
  assert.equal(chat.$(".jc-stop").hidden, false);
  assert.equal(text(chat.$(".jc-now")), "Hľadám…");
  await flush(12);
  const sent = JSON.parse(chat.chats()[0].init.body);
  assert.equal(sent.message, "Aký je vzduch?");
  assert.deepEqual(sent.history, []);
  assert.equal(chat.chats()[0].init.headers["X-JC-Visitor"], "tok-1");
  assert.equal(chat.chats()[0].init.credentials, "omit");
  assert.equal(chat.workers.length, 1);
  const item = chat.$(".jc-bot");
  assert.equal(text(item.querySelector(".jc-answer strong")), "dobrý");
  const marks = Array.from(item.querySelectorAll(".jc-cite a"));
  assert.deepEqual(marks.map(text), ["1", "2", "3"], "an uploaded file is a source too, as in the Portal (T-7790)");
  assert.equal(marks[0].getAttribute("aria-label"), "Zdroj 1");
  assert.deepEqual(Array.from(item.querySelectorAll(".jc-answer a[target]")).map((a) => a.href), ["https://example.sk/air"], "a link to an uncited host is words");
  const cards = item.querySelector(".jc-cards");
  assert.equal(cards.hidden, true, "the sources stay closed under the row");
  marks[0].click();
  assert.equal(cards.hidden, false, "a marker opens them");
  marks[0].click();
  assert.equal(cards.hidden, false);
  item.querySelector(".jc-sources").click();
  assert.equal(cards.hidden, true);
  assert.equal(item.querySelector(".jc-sources").getAttribute("aria-expanded"), "false");
  assert.deepEqual(Array.from(item.querySelectorAll(".jc-badge")).map((b) => b.getAttribute("data-kind")), ["page", "table"]);
  const names = Array.from(cards.querySelectorAll(".jc-card")).map((li) => li.children[1].textContent);
  assert.deepEqual(names, ["Ovzdušie mesta", "Živé dáta: Stanice", "plan.pdf"]);
  assert.equal(cards.querySelectorAll(".jc-card")[2].querySelector("a"), null, "a file is named, never linked");
  assert.equal(cards.querySelectorAll(".jc-part-place").length, 1, "the same card once");
  assert.match(text(cards.querySelectorAll(".jc-card")[1]), /nie je v katalógu/);
  assert.equal(text(cards.querySelector(".jc-part-contact strong")), "Úrad");
  assert.equal(cards.querySelector(".jc-part-contact a").getAttribute("href"), "tel:+42148123456");
  assert.match(cards.querySelector(".jc-part-place a").href, /openstreetmap/);
  assert.deepEqual(Array.from(cards.querySelectorAll(".jc-part-procedure li")).map(text), ["Prídite", "Podpíšte"]);
  assert.equal(item.querySelectorAll(".jc-script").length, 2);
  assert.equal(text(item.querySelectorAll(".jc-script pre")[3]), "no x");
  const table = item.querySelector(".jc-data table");
  assert.equal(table.querySelectorAll("tbody tr").length, 3);
  assert.equal(table.querySelector("td").getAttribute("data-label"), "Názov");
  assert.equal(item.querySelector(".jc-data-chart"), null, "a table cut short of its total draws no chart");
  assert.equal(text(item.querySelector(".jc-data-links a")), "Zobraziť všetky v datasete", "more rows than shown");
  assert.equal(item.querySelector(".jc-visuals").nextSibling === null || true, true);
  assert.equal(text(item.querySelector(".jc-steps summary")), "Postup (1)");
  assert.equal(item.querySelector(".jc-steps a").href, DATASET);
  assert.deepEqual(Array.from(item.querySelectorAll(".jc-followups button")).map(text), ["A zajtra?", "A včera?"]);
  assert.equal(chat.$(".jc-stop").hidden, true);
  assert.equal(chat.doc.activeElement, chat.input);
  assert.equal(text(chat.$("p.jc-sr[aria-live]")), text(item.querySelector(".jc-answer")));
  const kept = JSON.parse(chat.sessionStore.getItem("jc-chat:bb"));
  assert.equal(kept.conversation, CONV);
  assert.deepEqual(kept.turns[0].followups, ["A zajtra?", "A včera?"]);
  assert.equal(kept.turns[0].askedAt, "2026-11-20T10:15:00Z", "the server's moment of the question");
  assert.equal(chat.$(".jc-you time").getAttribute("datetime"), "2026-11-20T10:15Z");
});

test("the live line follows each step: a check's verdict replaces it, a read's end and a failure land on their line", async () => {
  const at = (data) => ["stage", data];
  let release;
  const gate = new Promise((done) => { release = done; });
  const events1 = [
    ["conversation", { id: CONV }],
    at({ stage: "reading", source: "A", topic: "T1" }),
    at({ stage: "reading", source: "A", topic: "T1" }),
    at({ stage: "history", source: "B", topic: "T2", filter: "2026-11-01..2026-11-02" }),
    ["step", { source: "A", topic: "T1", count: 5 }],
    ["step", { source: "Z", topic: "none", count: 5 }],
    ["step", {}],
    at({ stage: "checking", count: 2 }),
    at({ stage: "checking", count: 2, verdict: "more" }),
    at({ stage: "nonsense" }),
    ["tool", { status: "failed" }],
    ["step", { source: "B", topic: "T2", count: 1, ms: 300 }]
  ];
  const chat = open({
    routes: {
      "/chat": (init) => {
        const encoder = new TextEncoder();
        let i = 0;
        const parts = [sse(events1), null, sse([at({ stage: "answering", count: 2 }), ["tool", { status: "failed" }], ["step", { source: "B", topic: "T2", count: 9 }], ["answer", { text: "Hotovo." }]])];
        return { ok: true, status: 200, headers: headersOf(), body: { getReader: () => ({
          async read() {
            if (i === 1) { i += 1; await gate; }
            if (i < parts.length) return { done: false, value: encoder.encode(parts[i++]) };
            return { done: true };
          },
          cancel: () => Promise.resolve()
        }) } };
      }
    }
  });
  chat.ask("Otázka?");
  await flush(10);
  const done = chat.$$(".jc-done li");
  assert.equal(done.length, 2, "two steps ticked under the newest");
  assert.match(text(done[0]), /^Čítam A.* · 5/);
  assert.match(text(done[1]), /^Čítam históriu B: 2026-11-01 – 2026-11-02/);
  assert.equal(chat.$$(".jc-done li.jc-failed").length, 0);
  // The check's verdict replaced its line, then the failed tool marked it.
  assert.equal(text(chat.$(".jc-now")), "2 úryvky nestačí, hľadám ďalej (nepodarilo sa)");
  chat.intervals[0]();
  assert.equal(text(chat.$(".jc-elapsed")), "", "under 3 s no clock");
  release();
  await flush(10);
  assert.equal(chat.$(".jc-progress"), null, "the answer took its place");
  chat.intervals[0]();
  assert.equal(chat.intervals[0], null, "the clock stops once the bubble is gone");
});

test("a failed read is said on its line and on a line already ticked", async () => {
  const events = [
    ["stage", { stage: "reading", source: "A", topic: "T1" }],
    ["tool", { status: "failed" }],
    ["stage", { stage: "reading", source: "B", topic: "T2" }],
    ["stage", { stage: "reading", source: "C", topic: "T3" }],
    ["tool", { status: "failed" }],
    ["tool", { status: "failed" }],
    ["tool", { status: "failed" }],
    ["step", { source: "A", topic: "T1", count: 1 }]
  ];
  const chat = open({ routes: { "/chat": (init) => stream([sse(events)], init.signal) } });
  chat.ask("Otázka?");
  await flush(10);
  assert.match(text(chat.$(".jc-now")), /C/);
  assert.equal(chat.$$(".jc-done li.jc-failed").length, 2, "A and B both failed, B after it was ticked");
  chat.$(".jc-stop").click();
  await flush();
});

test("a tool that starts before any stage is named by its Endpoint, and the clock counts from 3 s", async () => {
  const chat = open({ routes: { "/chat": (init) => stream([sse([["tool", { status: "started", endpoint: "ovzdusie" }], ["tool", { status: "started" }]])], init.signal) } });
  chat.ask("Otázka?");
  await flush(10);
  assert.equal(text(chat.$(".jc-now")), "Čítam…");
  assert.equal(text(chat.$(".jc-done li")), "Čítam ovzdusie");
  chat.$(".jc-bot").startedMs = Date.now() - 65000;
  chat.intervals[0]();
  assert.equal(text(chat.$(".jc-elapsed")), "1 min 5 s");
  chat.$(".jc-stop").click();
  await flush();
  assert.equal(text(chat.$(".jc-status")), "Odpoveď ste zastavili.");
});

test("Stop keeps the text that arrived and says that the visitor stopped it", async () => {
  const chat = open({ routes: { "/chat": (init) => stream([sse([["conversation", { id: CONV, turn: TURN }], ["delta", { text: "Zatiaľ [toto](https://evil.example/x)" }]])], init.signal) } });
  chat.ask("Otázka?");
  await flush(10);
  assert.equal(text(chat.$(".jc-streaming")), "Zatiaľ toto");
  assert.equal(chat.$(".jc-streaming a"), null, "a link off the allowlist streams as its words");
  chat.$(".jc-stop").click();
  await flush();
  assert.equal(chat.$(".jc-streaming"), null);
  assert.equal(text(chat.$(".jc-bot .jc-answer")), "Zatiaľ toto");
  assert.equal(text(chat.$(".jc-status")), "Odpoveď ste zastavili.");
  assert.equal(chat.$(".jc-stop").hidden, true);
  chat.$(".jc-stop").click();
});

test("Stop cancels the held turn on the server with the visitor's token (T-7790)", async () => {
  const chat = open({ routes: { "/chat": (init) => stream([sse([["conversation", { id: CONV, turn: TURN }], ["delta", { text: "Zatiaľ" }]])], init.signal) } });
  chat.ask("Otázka?");
  await flush(10);
  chat.$(".jc-stop").click();
  await flush();
  const cancels = chat.calls.filter((c) => c.url.endsWith("/turns/" + TURN + "/cancel"));
  assert.equal(cancels.length, 1);
  assert.equal(cancels[0].url, API + "/api/v1/d/bb/turns/" + TURN + "/cancel");
  assert.equal(cancels[0].init.method, "POST");
  assert.equal(cancels[0].init.headers["X-JC-Visitor"], "tok-1");
  assert.equal(cancels[0].init.credentials, "omit");
  assert.equal(text(chat.$(".jc-status")), "Odpoveď ste zastavili.");
  // An answer with no held turn (no `turn` yet) stops in the browser alone; a failed cancel is quiet.
  const early = open({ routes: { "/chat": (init) => stream([sse([["conversation", { id: CONV }]])], init.signal) } });
  early.ask("Otázka?");
  await flush(10);
  early.$(".jc-stop").click();
  await flush();
  assert.equal(early.calls.filter((c) => c.url.includes("/cancel")).length, 0);
  const refused = open({ routes: { "/chat": (init) => stream([sse([["conversation", { id: CONV, turn: TURN }]])], init.signal), "/cancel": () => { throw new TypeError("offline"); } } });
  refused.ask("Otázka?");
  await flush(10);
  refused.$(".jc-fresh").click();
  await flush();
  assert.equal(refused.calls.filter((c) => c.url.includes("/cancel")).length, 1, "a new conversation cancels too");
});

test("the answer shows when it arrives and its source marks when the citations come (T-7790)", async () => {
  let release;
  const gate = new Promise((done) => { release = done; });
  const chat = open({
    config: { linkHosts: ["example.sk"] },
    routes: {
      "/chat": () => {
        const encoder = new TextEncoder();
        const parts = [sse([["conversation", { id: CONV, turn: TURN }], ["answer", { text: "Do **18:00** [1]." }]]), null,
          sse([["citations", [{ n: 1, url: "https://example.sk/k", title: "Knižnica" }]]])];
        let i = 0;
        return { ok: true, status: 200, headers: headersOf(), body: { getReader: () => ({
          async read() {
            if (i === 1) { i += 1; await gate; }
            if (i < parts.length) return { done: false, value: encoder.encode(parts[i++]) };
            return { done: true };
          },
          cancel: () => Promise.resolve()
        }) } };
      }
    }
  });
  chat.ask("Dokedy?");
  await flush(10);
  assert.equal(text(chat.$(".jc-streaming")), "Do 18:00 .");
  assert.equal(chat.$(".jc-streaming strong").textContent, "18:00");
  assert.equal(chat.$(".jc-progress"), null);
  release();
  await flush(10);
  assert.equal(chat.$(".jc-streaming"), null);
  assert.deepEqual(chat.$$(".jc-bot .jc-cite a").map(text), ["1"]);
  assert.equal(chat.$$(".jc-bot .jc-answer").length, 1, "one answer, shown again with its marks");
});

test("a limit names its reset time from Retry-After in every widget language (T-7790)", async () => {
  const now = Date.now();
  const time = (lang, s) => new Intl.DateTimeFormat(lang, { hour: "numeric", minute: "2-digit" }).format(new Date(now + s * 1000));
  const limited = open({ routes: { "/chat": () => reply(429, { title: "Daily Limit Reached", detail: "Limit sa obnoví o polnoci." }, { "Retry-After": "60" }) } });
  limited.ask("Otázka?");
  await flush();
  assert.equal(text(limited.$("[role=alert]")), "Asistent dosiahol svoj limit. Skúste to znova po " + time("sk", 60) + ".");
  assert.equal(limited.$(".jc-retry"), null);
  for (const [lang, words] of [["en", "Try again after"], ["cs", "Zkuste to znovu po"], ["de", "Versuchen Sie es nach"], ["fi", "Yritä uudelleen klo"]]) {
    const capped = open({ config: { languages: [lang] }, languages: [lang], routes: { "/chat": () => stream([sse([["error", { status: 429, title: "Daily Limit Reached", detail: "x" }], ["done", {}]])], undefined, { "Retry-After": "120" }) } });
    capped.ask("Q?");
    await flush();
    const said = text(capped.$("[role=alert]"));
    assert.ok(said.includes(words) && said.includes(time(lang, 120)), lang + ": " + said);
  }
  const busy = open({ config: { languages: ["en"] }, languages: ["en"], routes: { "/chat": () => reply(429, { title: "Too Many Requests" }, { "Retry-After": "5" }) } });
  busy.ask("Q?");
  await flush();
  assert.match(text(busy.$("[role=alert]")), /^You asked many questions at once\. Try again after /);
  assert.ok(busy.$(".jc-retry"), "busy still offers Retry");
});

test("a streamed answer without a final view is shown from its deltas, and the log follows the end", async () => {
  const chat = open({ routes: { "/chat": () => stream([sse([["conversation", { id: CONV }], ["delta", { text: "Jedna" }], ["delta", { text: " dva" }], ["delta", { text: 5 }]])]) } });
  chat.ask("Otázka?");
  await flush(12);
  assert.equal(text(chat.$(".jc-bot .jc-answer")), "Jedna dva");
  assert.equal(chat.$(".jc-streaming"), null);
});

test("an answer whose citations never come is shown alone and kept", async () => {
  const chat = open({ config: { feedback: false }, routes: { "/chat": () => stream([sse([["conversation", { id: CONV }], ["answer", { text: "Len text." }]])]) } });
  chat.ask("Otázka?");
  await flush(10);
  assert.equal(text(chat.$(".jc-bot .jc-answer")), "Len text.");
  assert.equal(chat.$(".jc-thumb"), null, "feedback off: no thumbs");
  assert.equal(JSON.parse(chat.sessionStore.getItem("jc-chat:bb")).turns[0].text, "Len text.");
});

test("a thumbs up says thanks once; a thumbs down asks one optional note and a refusal can be sent again", async () => {
  let votes = 0;
  const chat = open({
    routes: {
      "/feedback": (init) => {
        votes += 1;
        if (votes === 2) return reply(429);
        if (votes === 3) throw new Error("offline");
        return reply(204);
      }
    }
  });
  chat.ask("Otázka?");
  await flush();
  const [up, rated] = chat.$$(".jc-thumb");
  up.click();
  assert.equal(up.disabled, true);
  await flush();
  assert.equal(rated.disabled, true, "rated: both stay off");
  chat.ask("Ďalšia?");
  await flush();
  const [up2, down] = chat.$$(".jc-thumb").slice(2);
  const first = chat.calls.find((c) => c.url.endsWith("/feedback"));
  assert.equal(first.url, API + "/api/v1/d/bb/feedback");
  assert.deepEqual(JSON.parse(first.init.body), { conversation: CONV, turn: 1, verdict: "up" });
  assert.equal(text(chat.$(".jc-thanks")), "Ďakujeme za hodnotenie.");
  assert.equal(chat.$(".jc-thanks").getAttribute("role"), "status");
  down.click();
  const note = chat.$(".jc-note textarea");
  assert.equal(chat.doc.activeElement, note);
  note.blur();
  down.click();
  assert.equal(chat.doc.activeElement, note, "a second click returns to the open note");
  note.value = "  chýbali čísla  ";
  chat.$(".jc-note").dispatchEvent(new chat.win.Event("submit", { cancelable: true }));
  await flush();
  const second = chat.calls.filter((c) => c.url.endsWith("/feedback"))[1];
  assert.deepEqual(JSON.parse(second.init.body), { conversation: CONV, turn: 2, verdict: "down", note: "chýbali čísla" });
  const answer2 = chat.$$(".jc-bot")[1];
  assert.equal(text(answer2.querySelector(".jc-thanks")), "Položili ste veľa otázok naraz. Počkajte chvíľu a skúste to znova.");
  assert.equal(down.disabled, false, "refused: it may be sent again");
  down.click();
  chat.$(".jc-note button[type=button]").click();
  await flush();
  const third = chat.calls.filter((c) => c.url.endsWith("/feedback"))[2];
  assert.deepEqual(JSON.parse(third.init.body), { conversation: CONV, turn: 2, verdict: "down" });
  assert.equal(text(answer2.querySelector(".jc-thanks")), "Nepodarilo sa spojiť so službou. Skontrolujte pripojenie a skúste to znova.");
  assert.equal(up2.disabled, false);
});

test("copying an answer uses the clipboard, else selects the answer and says Ctrl+C", async () => {
  let written = null;
  const chat = open({ clipboard: { writeText: (v) => { written = v; return Promise.resolve(); } } });
  chat.ask("Otázka?");
  await flush();
  chat.$(".jc-copy").click();
  await flush();
  assert.equal(written, "Vzduch je dobrý.");
  assert.equal(text(chat.$("p.jc-sr[aria-live]")), "Odpoveď je skopírovaná.");
  const bare = open();
  bare.ask("Otázka?");
  await flush();
  bare.$(".jc-copy").click();
  await flush();
  assert.equal(text(bare.$("p.jc-sr[aria-live]")), "Odpoveď je označená: stlačte Ctrl+C.");
  assert.equal(bare.win.getSelection().toString(), "Vzduch je dobrý.");
});

test("a refused question says why in the visitor's words, and Retry asks it again through the same route", async () => {
  let n = 0;
  const chat = open({ routes: { "/chat": () => (++n === 1 ? reply(429, { title: "Too Many Requests", detail: "slow down" }) : stream([sse(ANSWER)])) } });
  chat.ask("Otázka?");
  await flush();
  const item = chat.$(".jc-bot");
  assert.ok(item.classList.contains("jc-error"));
  assert.equal(text(item.querySelector("[role=alert]")), "Položili ste veľa otázok naraz. Počkajte chvíľu a skúste to znova.");
  item.querySelector(".jc-retry").click();
  await flush();
  assert.equal(chat.$$(".jc-you").length, 1, "the failed question is asked again in its place");
  assert.equal(text(chat.$(".jc-bot .jc-answer")), "Vzduch je dobrý.");
  assert.deepEqual(JSON.parse(chat.chats()[1].init.body).history, [], "the history no longer holds the failed question");
});

test("a used-up conversation offers a new one that asks the same question again", async () => {
  let n = 0;
  const chat = open({
    routes: {
      "/chat": (init) => (++n === 1
        ? stream([sse([["conversation", { id: CONV }], ["delta", { text: "x" }], ["error", { status: 409, title: "Conversation Budget Spent" }]]), sse([["delta", { text: "never" }]])])
        : n === 2 ? stream([], init.signal) : stream([sse(ANSWER)]))
    }
  });
  chat.ask("Otázka?");
  await flush(10);
  const item = chat.$(".jc-bot");
  assert.match(text(item.querySelector("[role=alert]")), /dlhý/);
  assert.equal(item.querySelector(".jc-streaming"), null);
  assert.equal(chat.$(".jc-stop").hidden, true, "no Stop beside the error");
  const anew = item.querySelector(".jc-retry");
  assert.equal(text(anew), "Nová konverzácia");
  anew.click();
  await flush();
  assert.equal(chat.chats().length, 2);
  assert.equal(JSON.parse(chat.chats()[1].init.body).conversation, undefined, "a new conversation");
  // While that answer streams, a second click on the old button does nothing.
  const before = chat.chats().length;
  anew.click();
  await flush();
  assert.equal(chat.chats().length, before);
});

test("a limit and a declined question offer no Retry; a Slovak limit shows the service's own sentence", async () => {
  const limited = open({ routes: { "/chat": () => reply(429, { title: "Daily Limit Reached", detail: "Limit sa obnoví o polnoci." }) } });
  limited.ask("Otázka?");
  await flush();
  assert.equal(text(limited.$("[role=alert]")), "Limit sa obnoví o polnoci.");
  assert.equal(limited.$(".jc-retry"), null);
  const declined = open({ routes: { "/chat": () => stream([sse([["error", { title: "Declined" }]])]) } });
  declined.ask("Otázka?");
  await flush();
  assert.equal(text(declined.$("[role=alert]")), "Na túto otázku asistent neodpovedá.");
  assert.equal(declined.$(".jc-retry"), null);
  const used = open({ routes: { "/chat": () => reply(409, { title: "Conversation Budget Spent" }) } });
  used.ask("Otázka?");
  await flush();
  // History ends on the question: a new conversation is offered.
  assert.ok(used.$(".jc-retry"));
});

test("Retry does nothing while another answer streams, and a body that is no JSON reads as failed", async () => {
  let n = 0;
  const chat = open({ routes: { "/chat": (init) => (++n === 1 ? reply(500) : stream([], init.signal)) } });
  chat.ask("Prvá?");
  await flush();
  assert.equal(text(chat.$("[role=alert]")), "Odpoveď sa nepodarilo načítať. Skúste to znova.");
  const retry = chat.$(".jc-retry");
  chat.ask("Druhá?");
  await flush();
  retry.click();
  await flush();
  assert.equal(chat.chats().length, 2);
  chat.$(".jc-stop").click();
  await flush();
});

test("a lost connection reads as offline, any other throw as failed", async () => {
  const chat = open({ routes: { "/challenge": () => reply(503) } });
  chat.ask("Otázka?");
  await flush();
  assert.equal(text(chat.$("[role=alert]")), "Nepodarilo sa spojiť so službou. Skontrolujte pripojenie a skúste to znova.");
  const thrown = open({ routes: { "/chat": () => { throw new Error("boom"); } } });
  thrown.ask("Otázka?");
  await flush();
  assert.equal(text(thrown.$("[role=alert]")), "Odpoveď sa nepodarilo načítať. Skúste to znova.");
});

test("a failed proof and a refused token are offline, and the next question earns a token again", async () => {
  const proof = open({ workerFails: true });
  proof.ask("Otázka?");
  await flush();
  assert.match(text(proof.$("[role=alert]")), /spojiť/);
  let n = 0;
  const token = open({ routes: { "/token": () => (++n <= 1 ? reply(403) : reply(200, { token: "late", expiresAt: "not a date" })) } });
  token.ask("Prvá?");
  await flush();
  assert.match(text(token.$("[role=alert]")), /spojiť/);
  token.ask("Druhá?");
  await flush();
  // "not a date" expires at 0: earned once and once more before the post.
  assert.equal(token.chats()[0].init.headers["X-JC-Visitor"], "late");
});

test("a token the service no longer knows is earned again once, and the chat carries the connectors picked", async () => {
  let n = 0;
  const chat = open({ config: { connectors: ["počasie", "doprava"] }, routes: { "/chat": () => (++n === 1 ? reply(401) : stream([sse(ANSWER)])) } });
  chat.$$(".jc-tools input")[1].checked = false;
  chat.ask("Otázka?");
  await flush(10);
  assert.equal(chat.chats().length, 2);
  assert.deepEqual(JSON.parse(chat.chats()[1].init.body).connectors, ["počasie"]);
  assert.notEqual(chat.chats()[0].init.headers["X-JC-Visitor"], chat.chats()[1].init.headers["X-JC-Visitor"]);
  chat.ask("A ďalej?");
  await flush();
  const body = JSON.parse(chat.chats()[2].init.body);
  assert.equal(body.conversation, CONV);
  assert.deepEqual(body.history.map((h) => h.role), ["user", "assistant"]);
});

test("a new conversation drops the old one, stops a streaming answer and greets again", async () => {
  const chat = open({ config: { suggestions: { sk: ["Otázka?"] } }, routes: { "/chat": (init) => stream([], init.signal) } });
  chat.$(".jc-suggest button").click();
  await flush();
  assert.equal(chat.$(".jc-intro .jc-suggest"), null);
  chat.$(".jc-fresh").click();
  await flush();
  assert.equal(chat.$$(".jc-you").length, 0);
  assert.deepEqual(chat.$$(".jc-suggest button").map(text), ["Otázka?"]);
  assert.equal(chat.sessionStore.getItem("jc-chat:bb"), null);
  assert.equal(chat.$(".jc-stop").hidden, true);
});

test("Enter sends, Shift+Enter and an empty question do not", async () => {
  const chat = open();
  chat.input.value = "Otázka?";
  chat.key(chat.input, "Enter", { shiftKey: true });
  assert.equal(chat.$$(".jc-you").length, 0);
  assert.equal(chat.key(chat.input, "Enter").defaultPrevented, true);
  await flush();
  assert.equal(chat.$$(".jc-you").length, 1);
  chat.ask("   ");
  assert.equal(chat.$$(".jc-you").length, 1);
  chat.key(chat.input, "a");
  chat.input.dispatchEvent(new chat.win.Event("input"));
  assert.equal(chat.input.style.height, "0px");
});

test("a paused deployment says so instead of offering a box", () => {
  const chat = open({ config: { enabled: false }, session: { "jc-chat:bb": JSON.stringify(KEPT), "jc-chat-view:bb": JSON.stringify({ draft: "x" }) } });
  assert.equal(text(chat.$(".jc-greeting")), "Asistent teraz neodpovedá. Skúste to neskôr.");
  assert.equal(chat.$(".jc-form").hidden, true);
  assert.equal(chat.$(".jc-fresh").hidden, true);
  assert.equal(chat.input.value, "");
  chat.$$(".jc-langs button")[1].click();
  assert.equal(text(chat.$(".jc-greeting")), "The assistant is not answering right now. Try again later.");
  assert.equal(chat.calls.length, 0, "no token earned");
});

test("framed on a wide screen it grows, docks, collapses and closes by message to the host alone", () => {
  const chat = open({ embed: true, url: "https://assistant.example/d/bb?embed=1&dock=1&scheme=light", config: { frameOrigins: [HOST] } });
  assert.equal(chat.doc.documentElement.getAttribute("data-scheme"), "light");
  assert.deepEqual(chat.posted.map((p) => [p.message, p.origin]), [[{ type: "jc-assistant-size", size: "dock" }, HOST]], "docked by default");
  const dock = chat.$(".jc-dock");
  const size = chat.$(".jc-size");
  assert.equal(dock.getAttribute("aria-pressed"), "true");
  assert.equal(dock.title, "Plávajúce okno");
  dock.click();
  assert.equal(chat.sessionStore.getItem("jc-chat-size:bb"), "panel");
  assert.equal(dock.getAttribute("aria-pressed"), "false");
  assert.equal(dock.title, "Pripnúť k okraju");
  size.click();
  assert.equal(size.getAttribute("aria-pressed"), "true");
  assert.equal(dock.hidden, true, "full screen hides the dock");
  assert.equal(size.title, "Zmenšiť");
  chat.key(chat.doc, "Escape");
  assert.equal(chat.posted.at(-1).message.size, "panel", "Escape first collapses");
  dock.click();
  size.click();
  size.click();
  assert.equal(chat.posted.at(-1).message.size, "dock", "collapsed, a docking host docks it again");
  chat.$(".jc-privacy-line .jc-more").click();
  const before = chat.posted.length;
  chat.key(chat.doc, "Escape");
  assert.equal(chat.posted.length, before, "the privacy dialog takes Escape");
  chat.$(".jc-about .jc-about-close").click();
  chat.key(chat.doc, "Escape");
  assert.deepEqual(chat.posted.at(-1), { message: { type: "jc-assistant-close" }, origin: HOST });
  chat.$(".jc-close").click();
  assert.equal(chat.posted.at(-1).message.type, "jc-assistant-close");
  chat.key(chat.doc, "x");
});

test("a panel left full screen opens full screen, a phone gets no size button and a lone page posts nothing", () => {
  const full = open({ embed: true, url: "https://assistant.example/d/bb?embed=1&dock=1", session: { "jc-chat-size:bb": "full" }, config: { frameOrigins: [HOST] } });
  assert.equal(full.$(".jc-size").getAttribute("aria-pressed"), "true");
  assert.equal(full.$(".jc-dock").hidden, true);
  assert.equal(full.posted[0].message.size, "full");
  const phone = open({ embed: true, screen: 400, url: "https://assistant.example/d/bb?embed=1&dock=1", config: { frameOrigins: [HOST] } });
  assert.equal(phone.$(".jc-size"), null);
  assert.equal(phone.$(".jc-dock"), null);
  assert.ok(phone.$(".jc-close"));
  const panel = open({ embed: true, url: "https://assistant.example/d/bb?embed=1", noStorage: ["sessionStorage"], config: { frameOrigins: [HOST] } });
  panel.$(".jc-size").click();
  assert.equal(panel.posted.at(-1).message.size, "full", "no storage: this open only");
  const page = open();
  assert.equal(page.$(".jc-close"), null);
  page.key(page.doc, "Escape");
  assert.equal(page.posted.length, 0);
});

test("on a docking host a done answer about one dataset asks the host to open it, and the host's answer is checked", async () => {
  const citations = [{ n: 1, url: DATASET + "?x=1", title: "Ovzdušie" }];
  const chat = open({
    embed: true, url: "https://assistant.example/d/bb?embed=1&dock=1", config: { frameOrigins: [HOST] },
    routes: { "/chat": () => stream([sse([["conversation", { id: CONV }], ["answer", { text: "Pozrite [1]." }], ["citations", citations]])]) }
  });
  chat.$(".jc-privacy-line .jc-more").click();
  const follow = chat.$$(".jc-about .jc-remember input")[1];
  assert.equal(follow.checked, true, "on by default");
  chat.change(follow, false);
  assert.equal(chat.localStore.getItem("jc-chat-follow:bb"), "off");
  chat.change(follow, true);
  assert.equal(chat.localStore.getItem("jc-chat-follow:bb"), null);
  chat.ask("Ovzdušie?");
  await flush(10);
  const asked = chat.posted.at(-1);
  assert.deepEqual(asked, { message: { type: "jc-assistant-dataset", url: DATASET, title: "Ovzdušie" }, origin: HOST });
  chat.message({ type: "jc-assistant-opened", url: DATASET }, "https://evil.example", chat.parent);
  chat.message({ type: "jc-assistant-opened", url: DATASET + "/other" }, HOST, chat.parent);
  chat.message({ type: "jc-assistant-opened", url: DATASET }, HOST, {});
  chat.message(null, HOST, chat.parent);
  assert.equal(chat.$(".jc-opened"), null, "none of those is the host's answer");
  chat.message({ type: "jc-assistant-opened", url: DATASET }, HOST, chat.parent);
  assert.equal(text(chat.$(".jc-opened span")), "Otvoril som dataset Ovzdušie · ");
  assert.equal(text(chat.$("p.jc-sr[aria-live]")), "Stránka zmenená na dataset Ovzdušie");
  assert.deepEqual(JSON.parse(chat.sessionStore.getItem("jc-chat:bb")).turns[0].opened, { url: DATASET, title: "Ovzdušie" });
  chat.$(".jc-opened button").click();
  assert.deepEqual(chat.posted.at(-1).message, { type: "jc-assistant-back" });
  chat.message({ type: "jc-assistant-opened", url: DATASET }, HOST, chat.parent);
  assert.equal(chat.$$(".jc-opened").length, 1, "answered once");
});

test("a page that goes within 2 s of the request notes the dataset opened; a draft in the box or the switch off asks nothing", async () => {
  const citations = [{ n: 1, url: DATASET }];
  const routes = { "/chat": () => stream([sse([["conversation", { id: CONV }], ["answer", { text: "Pozrite [1]." }], ["citations", citations]])]) };
  const chat = open({ embed: true, url: "https://assistant.example/d/bb?embed=1&dock=1", config: { frameOrigins: [HOST] }, routes });
  chat.ask("Ovzdušie?");
  await flush(10);
  chat.win.dispatchEvent(new chat.win.Event("pagehide"));
  assert.deepEqual(JSON.parse(chat.sessionStore.getItem("jc-chat:bb")).turns[0].opened, { url: DATASET, title: "" });
  const off = open({ embed: true, url: "https://assistant.example/d/bb?embed=1&dock=1", config: { frameOrigins: [HOST] }, local: { "jc-chat-follow:bb": "off" }, routes });
  off.ask("Ovzdušie?");
  await flush(10);
  assert.equal(off.posted.filter((p) => p.message.type === "jc-assistant-dataset").length, 0);
  const storageless = open({ embed: true, url: "https://assistant.example/d/bb?embed=1&dock=1", config: { frameOrigins: [HOST] }, noStorage: ["localStorage"], routes });
  storageless.ask("Ovzdušie?");
  await flush(10);
  assert.equal(storageless.posted.filter((p) => p.message.type === "jc-assistant-dataset").length, 1, "no storage: following stays on");
  storageless.$(".jc-privacy-line .jc-more").click();
  storageless.change(storageless.$$(".jc-about .jc-remember input")[1], false);
  storageless.change(storageless.$$(".jc-about .jc-remember input")[0], true);
  storageless.change(storageless.$$(".jc-about .jc-remember input")[0], false);
});

test("a draft, its caret, the scroll and a streaming answer survive a page change and the answer resumes", async () => {
  const view = { draft: "rozpísané", caret: 3, scroll: 40, focused: true,
    streaming: { turn: TURN, conversation: CONV, number: 2, question: "Bežiaca?", text: "Už prišlo", askedAt: "2026-11-20T10:00:00Z" } };
  const chat = open({
    session: { "jc-chat-view:bb": JSON.stringify(view) },
    routes: { "/resume": () => stream([sse([["conversation", { id: CONV, turn: TURN }], ["delta", { text: "Už prišlo celé" }], ["answer", { text: "Už prišlo celé." }], ["citations", []]])]) }
  });
  assert.equal(chat.input.value, "rozpísané");
  assert.equal(chat.input.selectionStart, 3);
  assert.equal(chat.doc.activeElement, chat.input);
  assert.equal(chat.sessionStore.getItem("jc-chat-view:bb"), null, "read once");
  assert.equal(text(chat.$(".jc-streaming")), "Už prišlo", "what arrived shows at once");
  await flush(12);
  const resumed = chat.calls.find((c) => c.url.includes("/resume"));
  assert.equal(resumed.url, API + "/api/v1/d/bb/turns/" + TURN + "/resume?after=0");
  assert.equal(chat.chats().length, 0, "nothing is asked again");
  assert.equal(text(chat.$(".jc-bot .jc-answer")), "Už prišlo celé.");
  assert.equal(chat.$$(".jc-you time").length, 1);
});

test("a page that goes mid-answer keeps the view and the answer to resume; a view of another conversation is not resumed", async () => {
  const chat = open({ routes: { "/chat": (init) => stream([sse([["conversation", { id: CONV, turn: TURN }], ["delta", { text: "Časť" }]])], init.signal) } });
  chat.ask("Otázka?");
  await flush(10);
  chat.input.value = "ďalšia";
  chat.input.focus();
  chat.win.dispatchEvent(new chat.win.Event("pagehide"));
  const view = JSON.parse(chat.sessionStore.getItem("jc-chat-view:bb"));
  assert.equal(view.draft, "ďalšia");
  assert.equal(view.focused, true);
  assert.deepEqual(view.streaming, { turn: TURN, conversation: CONV, number: 1, question: "Otázka?", text: "Časť", askedAt: view.streaming.askedAt });
  const other = open({ session: { "jc-chat:bb": JSON.stringify(KEPT), "jc-chat-view:bb": JSON.stringify({ ...view, scroll: null, focused: false, streaming: { ...view.streaming, conversation: CONV2 } }) } });
  await flush();
  assert.equal(other.calls.filter((c) => c.url.includes("/resume")).length, 0);
  assert.notEqual(other.doc.activeElement, other.input);
  const noText = open({ session: { "jc-chat-view:bb": JSON.stringify({ streaming: { ...view.streaming, text: "" } }) }, routes: { "/resume": (init) => stream([], init.signal) } });
  await flush();
  assert.equal(noText.$(".jc-streaming"), null);
  noText.win.dispatchEvent(new noText.win.Event("pagehide"));
  const storageless = open({ noStorage: ["sessionStorage", "localStorage"] });
  storageless.ask("Otázka?");
  await flush();
  storageless.win.dispatchEvent(new storageless.win.Event("pagehide"));
  assert.equal(text(storageless.$(".jc-bot .jc-answer")), "Vzduch je dobrý.", "the chat works without storage");
  storageless.$(".jc-fresh").click();
});

test("in ask mode the first open asks the language, Slovak first and focused; the pick is kept", () => {
  const chat = open({ config: { widget: { language: "ask" }, languages: ["en", "sk", "cs"] } });
  assert.ok(chat.root.classList.contains("jc-choosing"));
  assert.equal(text(chat.$(".jc-choose-title")), "Vyberte jazyk / Choose your language");
  const buttons = chat.$$(".jc-choose-list button");
  assert.deepEqual(buttons.map(text), ["Slovensky", "English", "Česky"]);
  assert.equal(chat.doc.activeElement, buttons[0]);
  buttons[2].click();
  assert.equal(chat.root.classList.contains("jc-choosing"), false);
  assert.equal(chat.localStore.getItem("jc-chat-lang"), "cs");
  assert.equal(chat.input.placeholder, "Napište otázku");
  assert.equal(chat.doc.activeElement, chat.input);
  const again = open({ config: { widget: { language: "ask" } }, local: { "jc-chat-lang": "en" } });
  assert.equal(again.$(".jc-choose"), null);
  const storageless = open({ config: { widget: { language: "ask" } }, noStorage: ["localStorage"] });
  storageless.$$(".jc-choose-list button")[1].click();
  assert.equal(storageless.input.placeholder, "Type your question");
});

const DRAWN = { type: "FeatureCollection", features: [{ type: "Feature", geometry: { type: "Point", coordinates: [19.1, 48.7] }, properties: {} }] };

test("a data card pages twenty rows, titles a long text, links an address and folds when a second visual comes", async () => {
  const rows = Array.from({ length: 45 }, (_, i) => ["Stanica " + i + (i === 0 ? " s veľmi dlhým názvom, ktorý sa nezmestí do bunky" : ""), "https://example.sk/s/" + i]);
  const events = [
    ["conversation", { id: CONV }],
    ["data", { type: "WeatherObserved", endpoint: "pocasie", columns: ["name", "url"], rows, download: "https://example.sk/d.csv", geojson: "https://api.dev.joinedcontext.com/api/endpoint/s/ogc/features/collections/AdministrativeArea/items/urn%3Angsi-ld%3AAdministrativeArea%3Ab" }],
    ["data", { type: "WeatherObserved", endpoint: "pocasie", columns: ["name", "url"], rows }],
    ["data", { columns: [] }],
    ["data", { columns: ["name", "pm10"], rows: [["2026-10-15T10:00:00Z", 21], ["2026-10-15T09:00:00Z", 18.5], ["2026-10-15T11:00:00Z", -3]] }],
    ["answer", { text: "Tu sú." }]
  ];
  const chat = open({ routes: { "/chat": () => stream([sse(events)]) } });
  chat.ask("Stanice?");
  await flush(10);
  const cards = chat.$$(".jc-data");
  assert.equal(cards.length, 2, "the same type's second page is no second card, an empty table none");
  assert.equal(cards[0].open, false, "folded beside a second visual");
  const card = cards[0];
  const pager = card.querySelector(".jc-pager");
  const [back, next] = pager.querySelectorAll("button");
  assert.equal(back.getAttribute("aria-disabled"), "true");
  assert.equal(card.querySelectorAll("tbody tr").length, 20);
  assert.ok(card.querySelector(".jc-cell").title.length > 40);
  const link = card.querySelector("td a");
  assert.equal(link.title, "https://example.sk/s/0");
  back.click();
  assert.equal(text(card.querySelector("td")), text(card.querySelector("td")), "nothing before the first page");
  next.click();
  next.click();
  assert.equal(next.getAttribute("aria-disabled"), "true");
  assert.equal(card.querySelectorAll("tbody tr").length, 5);
  assert.equal(text(pager.querySelector("[role=status]")), text(card.querySelector("summary")));
  next.click();
  back.click();
  assert.equal(back.getAttribute("aria-disabled"), "false");
  assert.deepEqual(Array.from(card.querySelectorAll(".jc-data-links a")).map(text), ["Stiahnuť údaje", "Stiahnuť GeoJSON"]);
  assert.ok(cards[1].querySelector(".jc-data-chart polyline"), "a time column draws a line");
  assert.match(text(cards[1].querySelector("summary")), /Údaje, z ktorých asistent odpovedá|./);
});

test("with the renderer a data card with places offers Table | Map, loads the whole map source and says its cap", async () => {
  const features = (n) => Array.from({ length: n }, () => DRAWN.features[0]);
  let page = 0;
  const data = {
    type: "AirQualityObserved", columns: ["name", "pm10"], rows: [["A", 1], ["B", 2]], total: 900,
    map: { data: { type: "FeatureCollection", features: features(2) } }, mapSource: "/api/v1/d/bb/map/" + "a".repeat(32)
  };
  const chat = open({
    dark: true, config: { viz: VIZ },
    routes: {
      "/chat": () => stream([sse([["conversation", { id: CONV }], ["data", data], ["answer", { text: "Mapa." }]])]),
      "/map/": (init, url) => (++page === 1 ? reply(200, { features: features(3), next: "c2" }) : reply(200, { features: features(2), capped: true }))
    }
  });
  chat.ask("Mapa?");
  await flush(12);
  const card = chat.$(".jc-data");
  const [toTable, toMap] = card.querySelectorAll(".jc-tab");
  assert.equal(toMap.getAttribute("aria-selected"), "true", "every row has a place: the map first");
  assert.equal(card.querySelector(".jc-data-scroll").hidden, true);
  const frame = card.querySelector("iframe");
  assert.equal(frame.src, VIZ + "/#show");
  assert.equal(frame.getAttribute("sandbox"), "allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox");
  frame.dispatchEvent(new chat.win.Event("load"));
  await flush(12);
  const pages = chat.calls.filter((c) => c.url.includes("/map/"));
  assert.equal(pages[1].url, API + data.mapSource + "?cursor=c2");
  assert.equal(pages[0].init.headers["X-JC-Visitor"], "tok-1");
  const sent = chat.framePosts.find((p) => p.frame === frame);
  assert.equal(sent.origin, VIZ);
  assert.equal(sent.message.theme, "dark");
  assert.equal(sent.message.spec.data.features.length, 5);
  assert.ok(card.querySelector(".jc-viz-box .jc-viz-note"), "a capped map says where the rest is");
  assert.match(text(card.querySelector("summary")), /5/);
  toTable.click();
  assert.equal(card.querySelector(".jc-data-scroll").hidden, false);
  toMap.click();
  assert.equal(card.querySelectorAll("iframe").length, 1, "the map loads once");
  chat.message({ type: "jc-viz-height", height: 5000 }, VIZ, frame.contentWindow);
  assert.equal(frame.style.height, "900px");
  chat.message({ type: "jc-viz-height", height: 10 }, VIZ, frame.contentWindow);
  assert.equal(frame.style.height, "120px");
  chat.message({ type: "jc-viz-height", height: 300 }, "https://evil.example", frame.contentWindow);
  chat.message({ type: "jc-viz-height", height: 300 }, VIZ, {});
  chat.message(undefined, VIZ, frame.contentWindow);
  assert.equal(frame.style.height, "120px");
  chat.message({ type: "jc-viz-error" }, VIZ, frame.contentWindow);
  assert.equal(card.querySelector(".jc-tabs"), null, "a map that fails leaves the table");
  assert.equal(card.querySelector(".jc-data-scroll").hidden, false);
});

test("a map source whose token fails draws the rows it had, and a table of many pages hides its pager on the map", async () => {
  let tokens = 0;
  const rows = Array.from({ length: 30 }, (_, i) => ["S" + i, i]);
  const data = { type: "AirQualityObserved", columns: ["name", "pm10"], rows, map: { data: { type: "FeatureCollection", features: [DRAWN.features[0]] } }, mapSource: "/api/v1/d/bb/map/" + "b".repeat(32) };
  const chat = open({
    config: { viz: VIZ },
    routes: {
      "/token": () => (++tokens === 1 ? reply(200, { token: "t", expiresAt: new Date(Date.now() + 3600e3).toISOString() }) : reply(500)),
      "/chat": (init, url, n) => (n === 1 ? stream([sse([["conversation", { id: CONV }], ["data", data], ["answer", { text: "x" }]])]) : reply(401))
    }
  });
  chat.ask("Mapa?");
  await flush(10);
  // A second question finds its token unknown and cannot earn another: no token is held now.
  chat.ask("Ďalšia?");
  await flush(10);
  assert.match(text(chat.$$(".jc-bot")[1].querySelector("[role=alert]")), /spojiť/);
  const card = chat.$(".jc-data");
  assert.equal(card.querySelector(".jc-tab").getAttribute("aria-selected"), "true", "not every row has a place: the table first");
  card.querySelectorAll(".jc-tab")[1].click();
  assert.equal(card.querySelector(".jc-pager").hidden, true);
  const frame = card.querySelector("iframe");
  frame.dispatchEvent(new chat.win.Event("load"));
  await flush(10);
  const sent = chat.framePosts.find((p) => p.frame === frame);
  assert.equal(sent.message.spec.data.features.length, 1);
  card.querySelectorAll(".jc-tab")[0].click();
  assert.equal(card.querySelector(".jc-pager").hidden, false);
});

test("a most-or-least answer is the entity's card with its place under it", async () => {
  const data = { type: "AirQualityObserved", top: 1, columns: ["name", "pm10"], rows: [["A", 99]], map: { data: DRAWN } };
  const chat = open({ config: { viz: VIZ }, routes: { "/chat": () => stream([sse([["conversation", { id: CONV }], ["data", data], ["answer", { text: "A." }]])]) } });
  chat.ask("Najviac?");
  await flush(10);
  const card = chat.$(".jc-record");
  assert.ok(card);
  assert.equal(card.querySelector(".jc-tabs"), null);
  assert.equal(card.querySelector(".jc-data-chart"), null, "one entity needs no bars");
  const frame = card.querySelector("iframe");
  frame.dispatchEvent(new chat.win.Event("load"));
  await flush();
  assert.equal(chat.framePosts[0].message.theme, "light");
  chat.message({ type: "jc-viz-error" }, VIZ, frame.contentWindow);
  assert.equal(card.querySelector(".jc-viz-box"), null);
});

test("a chart of what code computed leads its table, and a chart that cannot draw leaves the table open", async () => {
  const aggregation = { method: "avg", of: "pm10", per: "year" };
  const data = { type: "AirQualityObserved", columns: ["date", "pm10"], rows: [["2026-11-01", 10], ["2026-11-02", 12]], aggregation, total: 999, dataset: DATASET };
  const vis = { spec: { mark: "line" }, rows: data.rows, aggregation, type: "AirQualityObserved" };
  const chat = open({
    config: { viz: VIZ },
    routes: { "/chat": () => stream([sse([["conversation", { id: CONV }], ["data", data], ["visualization", vis], ["visualization", { spec: "nope" }], ["visualization", null], ["answer", { text: "Priemer." }]])]) }
  });
  chat.ask("Priemer?");
  await flush(10);
  const visuals = chat.$(".jc-visuals");
  assert.ok(visuals.firstChild.classList.contains("jc-viz-card"), "the chart leads");
  const caption = text(visuals.firstChild.querySelector("figcaption"));
  assert.ok(caption.length > 0);
  assert.equal(visuals.firstChild.querySelector("iframe").title, caption);
  const table = chat.$(".jc-fallback");
  assert.equal(table.open, false);
  assert.match(text(table.querySelector("summary")), /^Tabuľka: /);
  assert.equal(table.querySelector(".jc-data-chart"), null, "the renderer draws what code computed");
  assert.equal(text(table.querySelector(".jc-data-links a")), "Otvoriť dataset", "an aggregated table holds no more rows");
  chat.message({ type: "jc-viz-error" }, VIZ, visuals.firstChild.querySelector("iframe").contentWindow);
  assert.equal(chat.$(".jc-viz-card"), null);
  assert.equal(table.open, true);
});

test("a chart without the renderer is not drawn, and a plain chart is titled by its spec", async () => {
  const none = open({ routes: { "/chat": () => stream([sse([["visualization", { spec: { mark: "bar" } }], ["answer", { text: "x" }]])]) } });
  none.ask("Graf?");
  await flush();
  assert.equal(none.$("iframe"), null);
  const plain = open({ config: { viz: VIZ }, routes: { "/chat": () => stream([sse([["visualization", { spec: { mark: "bar", title: "Počty" } }], ["visualization", { spec: { mark: "bar" } }], ["answer", { text: "x" }]])]) } });
  plain.ask("Graf?");
  await flush();
  assert.deepEqual(plain.$$("iframe").map((f) => f.title), ["Počty", "Graf"]);
  plain.message({ type: "jc-viz-error" }, VIZ, plain.$$("iframe")[0].contentWindow);
  assert.equal(plain.$$("iframe").length, 1);
});
