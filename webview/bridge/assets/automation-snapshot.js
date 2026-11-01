(() => {
  const title = String(document.title || "").replace(/\s+/g, " ").trim().slice(0, 500);
  const url = String(document.URL || document.location && document.location.href || "").slice(0, 2048);
  const frames = [];
  const queue = [{ doc: document, path: "0", depth: 0 }];
  let totalNodes = 0;
  const maxNodes = 20000;
  const maxFrames = 64;
  const maxFrameDepth = 16;

  while (queue.length && frames.length < maxFrames) {
    const current = queue.shift();
    if (current.inaccessible) {
      frames.push({
        path: current.path,
        depth: current.depth,
        inaccessible: true,
        nodes: [{ role: "iframe", name: current.name, unreadable: true }],
      });
      continue;
    }
    let api = current.doc.defaultView && current.doc.defaultView.__egoAutomation;
    if (!api) {
      try {
        const frameWindow = current.doc.defaultView;
        const install = window.__egoInstallAutomation;
        if (frameWindow && typeof install === "function") api = install(frameWindow);
      } catch (_) {
        api = null;
      }
    }
    if (!api) {
      frames.push({ path: current.path, depth: current.depth, nodes: [{ role: "document", name: "automation preload unavailable", unreadable: true }] });
      continue;
    }
    const result = api.localSnapshot(current.doc, maxNodes - totalNodes);
    totalNodes += result.nodeCount;
    const frame = { path: current.path, depth: current.depth, nodes: result.nodes };
    if (result.truncated) frame.truncated = true;
    frames.push(frame);
    if (current.depth >= maxFrameDepth) {
      if (current.doc.querySelectorAll("iframe,frame").length) {
        frame.truncated = true;
        frame.nodes.push({ role: "status", name: "[frame depth limit reached]" });
      }
      continue;
    }

    const frameElements = current.doc.querySelectorAll("iframe,frame");
    const availableFrames = Math.max(0, maxFrames - frames.length - queue.length);
    const childLimit = Math.min(frameElements.length, availableFrames);
    for (let index = 0; index < childLimit; index++) {
      const element = frameElements[index];
      const path = current.path + "." + index;
      let child = null;
      try {
        child = element.contentDocument;
        if (!child && element.contentWindow) child = element.contentWindow.document;
      } catch (_) {
        child = null;
      }
      if (child) {
        queue.push({ doc: child, path, depth: current.depth + 1 });
      } else {
        const name = element.getAttribute("title") || element.getAttribute("name") || "";
        queue.push({ path, depth: current.depth + 1, name: String(name).slice(0, 500), inaccessible: true });
      }
    }
    if (childLimit < frameElements.length) {
      frame.truncated = true;
      frame.nodes.push({ role: "status", name: "[frame count limit reached]" });
    }
  }
  if (queue.length) {
    frames.push({ path: "[truncated]", depth: 0, nodes: [{ role: "status", name: "[frame limit reached]", unreadable: true }], truncated: true });
  }
  return { title, url, frames };
})()
