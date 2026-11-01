(() => {
  const reference = __EGO_REFERENCE__;
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
  if (element.disabled) throw new Error("target is disabled");
  if (typeof element.click !== "function") throw new Error("target cannot be clicked");
  element.click();
  return true;
})()
