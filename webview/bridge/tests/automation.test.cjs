const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const assets = path.join(__dirname, "../assets");
const read = name => fs.readFileSync(path.join(assets, name), "utf8");
const preload = read("automation-preload.js");
const snapshotScript = read("automation-snapshot.js");

class TextNode {
  constructor(text) { this.nodeType = 3; this.nodeValue = text; this.textContent = text; }
}

class Element {
  constructor(tag, attrs = {}, text = "") {
    this.tagName = tag.toUpperCase();
    this.attributes = attrs;
    this.childNodes = [];
    this.children = [];
    this.parentElement = null;
    this.isConnected = true;
    this.disabled = false;
    this.readOnly = false;
    this.events = [];
    this.contentEditable = false;
    if (text) this.append(new TextNode(text));
    if (attrs.value !== undefined) this.value = attrs.value;
    if (attrs.type) this.type = attrs.type;
  }
  append(child) {
    this.childNodes.push(child);
    if (child instanceof Element) {
      child.parentElement = this;
      this.children.push(child);
    }
    return child;
  }
  getAttribute(name) { return Object.hasOwn(this.attributes, name) ? this.attributes[name] : null; }
  get textContent() { return this.childNodes.map(child => child.textContent || "").join(""); }
  set textContent(value) { this.childNodes = [new TextNode(String(value))]; this.children = []; }
  get innerText() { return this.textContent; }
  click() { this.clicked = true; }
  focus() { this.focused = true; }
  dispatchEvent(event) { this.events.push(event.type); return true; }
  querySelectorAll(selector) {
    const found = [];
    const stack = [...this.children].reverse();
    while (stack.length) {
      const item = stack.pop();
      if (selector === "*" || selector === "iframe,frame" && ["IFRAME", "FRAME"].includes(item.tagName)) found.push(item);
      stack.push(...[...item.children].reverse());
    }
    return found;
  }
}

class Input extends Element {
  constructor(attrs = {}) { super("input", attrs); this._value = attrs.value || ""; }
  get value() { return this._value; }
  set value(value) { this._value = String(value); }
}

class Document {
  constructor() {
    this.body = new Element("body");
    this.documentElement = this.body;
    this.title = "Fixture";
    this.URL = "https://example.test/";
    this.defaultView = null;
  }
  querySelectorAll(selector) { return this.body.querySelectorAll(selector); }
}

class DOMEvent { constructor(type, options = {}) { this.type = type; Object.assign(this, options); } }

function createWindow(doc) {
  const win = { document: doc, Event: DOMEvent, InputEvent: DOMEvent };
  doc.defaultView = win;
  vm.runInNewContext(preload, { window: win, document: doc });
  return win;
}

function run(win, script) {
  return vm.runInNewContext(script, { window: win, document: win.document });
}

function script(name, values) {
  let source = read(name);
  for (const [marker, value] of Object.entries(values)) source = source.replace(marker, JSON.stringify(value));
  return source;
}

test("walker assigns stable frame-scoped refs and reports inaccessible frames", () => {
  const top = new Document();
  const button = top.body.append(new Element("button", {}, "Open"));
  const secret = top.body.append(new Input({ type: "password", value: "do-not-snapshot" }));
  const sameOriginFrame = top.body.append(new Element("iframe"));
  const child = new Document();
  const input = child.body.append(new Input({ placeholder: "Name" }));
  sameOriginFrame.contentDocument = child;
  sameOriginFrame.contentWindow = { document: child, Event: DOMEvent, InputEvent: DOMEvent };
  child.defaultView = sameOriginFrame.contentWindow;
  assert.equal(child.defaultView.__egoAutomation, undefined);
  const crossOriginFrame = top.body.append(new Element("iframe", { title: "Private" }));
  Object.defineProperty(crossOriginFrame, "contentDocument", { get() { throw new Error("cross origin"); } });
  Object.defineProperty(crossOriginFrame, "contentWindow", { get() { return { get document() { throw new Error("cross origin"); } }; } });
  const win = createWindow(top);

  const first = run(win, snapshotScript);
  const second = run(win, snapshotScript);
  assert.match(first.frames[0].nodes[0].ref, /^e[0-9a-z]+-1$/);
  assert.equal(first.frames[0].nodes[0].ref, second.frames[0].nodes[0].ref);
  assert.equal(first.frames[0].nodes[1].value, undefined);
  assert.equal(first.frames[1].path, "0.0");
  assert.match(first.frames[1].nodes[0].ref, /^e[0-9a-z]+-1$/);
  assert.equal(first.frames[1].nodes[0].ref, second.frames[1].nodes[0].ref);
  assert.notEqual(first.frames[0].nodes[0].ref, first.frames[1].nodes[0].ref);
  assert.equal(typeof child.defaultView.__egoAutomation.localSnapshot, "function");
  assert.equal(first.frames[2].inaccessible, true);
  assert.equal(first.frames[2].nodes[0].unreadable, true);

  const reference = "0.0:" + first.frames[1].nodes[0].ref;
  assert.equal(run(win, script("automation-fill.js", {
    "__EGO_REFERENCE__": reference,
    "__EGO_VALUE__": "escaped ' text\nvalue",
  })), true);
  assert.equal(input.value, "escaped ' text\nvalue");
  assert.deepEqual(input.events, ["input", "change"]);

  const buttonRef = "0:" + first.frames[0].nodes[0].ref;
  assert.equal(run(win, script("automation-click.js", { "__EGO_REFERENCE__": buttonRef })), true);
  assert.equal(button.clicked, true);
});

test("references from a replaced same-origin document do not resolve in its replacement", () => {
  const top = new Document();
  const frame = top.body.append(new Element("iframe"));
  const originalDocument = new Document();
  originalDocument.body.append(new Element("button", {}, "Original"));
  const originalWindow = createWindow(originalDocument);
  frame.contentDocument = originalDocument;
  frame.contentWindow = originalWindow;
  const win = createWindow(top);

  const originalRef = run(win, snapshotScript).frames[1].nodes[0].ref;
  const replacementDocument = new Document();
  replacementDocument.body.append(new Element("button", {}, "Replacement"));
  const replacementWindow = { document: replacementDocument, Event: DOMEvent, InputEvent: DOMEvent };
  replacementDocument.defaultView = replacementWindow;
  frame.contentDocument = replacementDocument;
  frame.contentWindow = replacementWindow;
  run(win, snapshotScript);

  assert.throws(() => run(win, script("automation-click.js", {
    "__EGO_REFERENCE__": "0.0:" + originalRef,
  })), /stale or unknown/);
});

test("stale refs, invalid refs, and non-editable fill targets fail explicitly", () => {
  const doc = new Document();
  const button = doc.body.append(new Element("button", {}, "Go"));
  const win = createWindow(doc);
  const snapshot = run(win, snapshotScript);
  const reference = "0:" + snapshot.frames[0].nodes[0].ref;
  button.isConnected = false;
  assert.throws(() => run(win, script("automation-click.js", { "__EGO_REFERENCE__": reference })), /stale or unknown/);
  assert.throws(() => run(win, script("automation-click.js", { "__EGO_REFERENCE__": "bad" })), /invalid automation reference/);

  button.isConnected = true;
  const refreshed = run(win, snapshotScript).frames[0].nodes[0].ref;
  assert.throws(() => run(win, script("automation-fill.js", {
    "__EGO_REFERENCE__": "0:" + refreshed,
    "__EGO_VALUE__": "x",
  })), /not an editable field/);
});
