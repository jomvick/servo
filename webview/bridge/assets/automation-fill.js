(() => {
  const reference = __EGO_REFERENCE__;
  const value = __EGO_VALUE__;
  const separator = reference.lastIndexOf(":");
  const path = separator < 0 ? "" : reference.slice(0, separator);
  const localReference = separator < 0 ? "" : reference.slice(separator + 1);
  if (!/^0(?:\.\d+)*$/.test(path) || !/^e[0-9a-z]+-[1-9]\d*$/.test(localReference)) {
    throw new Error("invalid automation reference");
  }
  let doc = document;
  const segments = path.split(".").slice(1);
  for (const segment of segments) {
    const frame = doc.querySelectorAll("iframe,frame")[Number(segment)];
    if (!frame) throw new Error("stale frame reference");
    try {
      doc = frame.contentDocument || frame.contentWindow && frame.contentWindow.document;
    } catch (_) {
      throw new Error("frame is inaccessible");
    }
    if (!doc) throw new Error("frame is inaccessible");
  }
  const api = doc.defaultView && doc.defaultView.__egoAutomation;
  if (!api) throw new Error("automation preload unavailable in frame");
  const element = api.resolve(localReference);
  const tag = String(element.tagName || "").toLowerCase();
  const type = String(element.type || "text").toLowerCase();
  if (!(tag === "textarea" || tag === "select" || tag === "input" &&
      !["button", "checkbox", "color", "file", "hidden", "image", "radio", "range", "reset", "submit"].includes(type) ||
      element.isContentEditable === true)) {
    throw new Error("target is not an editable field");
  }
  if (element.disabled || element.readOnly) throw new Error("target is disabled or read-only");
  if (tag === "select" && !Array.from(element.options || []).some(option => option.value === value)) {
    throw new Error("select has no option with the requested value");
  }
  if (typeof element.focus === "function") element.focus();
  if (element.isContentEditable === true) {
    element.textContent = value;
  } else {
    let prototype = Object.getPrototypeOf(element);
    let setter = null;
    while (prototype && !setter) {
      const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
      if (descriptor && descriptor.set) setter = descriptor.set;
      prototype = Object.getPrototypeOf(prototype);
    }
    if (setter) setter.call(element, value);
    else element.value = value;
  }
  const win = doc.defaultView;
  try {
    element.dispatchEvent(new win.InputEvent("input", { bubbles: true, inputType: "insertText", data: value }));
  } catch (_) {
    element.dispatchEvent(new win.Event("input", { bubbles: true }));
  }
  element.dispatchEvent(new win.Event("change", { bubbles: true }));
  return true;
})()
