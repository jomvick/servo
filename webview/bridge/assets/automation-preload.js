(() => {
  function install(win) {
    if (!win || win.__egoAutomation) return win && win.__egoAutomation;

    const references = new WeakMap();
    const elements = new Map();
    let nextReference = 0;
    const maxFieldLength = 500;
    let documentNonce;
    try {
      const random = new Uint32Array(4);
      win.crypto.getRandomValues(random);
      documentNonce = Array.from(random, word => word.toString(36)).join("");
    } catch (_) {
      documentNonce = Math.random().toString(36).slice(2) + Date.now().toString(36);
    }

  function bounded(value) {
    return String(value || "").replace(/\s+/g, " ").trim().slice(0, maxFieldLength);
  }

  function attribute(element, name) {
    return element.getAttribute ? element.getAttribute(name) : null;
  }

  function roleFor(element) {
    const explicit = attribute(element, "role");
    if (explicit) return explicit;
    const tag = String(element.tagName || "").toLowerCase();
    if (tag === "a" && attribute(element, "href")) return "link";
    if (tag === "button" || tag === "summary") return "button";
    if (tag === "textarea") return "textbox";
    if (tag === "select") return "combobox";
    if (tag === "input") {
      const type = String(element.type || "text").toLowerCase();
      return type === "checkbox" ? "checkbox" : type === "radio" ? "radio" : "textbox";
    }
    return tag || "text";
  }

  function isActionable(element) {
    const tag = String(element.tagName || "").toLowerCase();
    return tag === "a" && !!attribute(element, "href") ||
      tag === "button" || tag === "input" || tag === "textarea" || tag === "select" ||
      tag === "summary" || element.isContentEditable === true ||
      attribute(element, "onclick") !== null || attribute(element, "tabindex") !== null ||
      ["button", "link", "checkbox", "radio", "textbox", "combobox", "switch"].includes(attribute(element, "role"));
  }

  function referenceFor(element) {
      let reference = references.get(element);
      if (!reference) {
        reference = "e" + documentNonce + "-" + (++nextReference);
      references.set(element, reference);
      elements.set(reference, element);
    }
    return reference;
  }

  function nameFor(element, actionable) {
    const label = attribute(element, "aria-label") || attribute(element, "alt") ||
      attribute(element, "placeholder") || attribute(element, "title");
    if (label) return bounded(label);
    if (actionable) return bounded(element.innerText || element.textContent || "");
    let text = "";
    for (const child of Array.from(element.childNodes || [])) {
      if (child.nodeType === 3) text += " " + (child.nodeValue || child.textContent || "");
    }
    return bounded(text);
  }

  function localSnapshot(doc, limit) {
    for (const [reference, element] of elements) {
      if (element.isConnected === false) {
        elements.delete(reference);
        references.delete(element);
      }
    }
    const budget = Math.max(0, Math.min(Number(limit) || 0, 20000));
    const root = doc.body || doc.documentElement;
    const nodes = [];
    if (!root) {
      return { nodes, nodeCount: 0, truncated: false };
    }
    if (budget === 0) {
      nodes.push({ role: "status", name: "[snapshot node limit reached]" });
      return { nodes, nodeCount: 0, truncated: true };
    }

    const stack = [];
    const children = root.children || [];
    const rootCount = Math.min(children.length, budget);
    for (let i = rootCount - 1; i >= 0; i--) {
      stack.push({ element: children[i], parent: nodes, depth: 0 });
    }
    let visited = 0;
    let truncated = rootCount < children.length;
    while (stack.length) {
      if (visited >= budget) {
        truncated = true;
        break;
      }
      const current = stack.pop();
      visited++;
      const element = current.element;
      const tag = String(element.tagName || "").toLowerCase();
      if (["script", "style", "noscript", "template"].includes(tag)) continue;

      const actionable = isActionable(element);
      const node = {
        role: roleFor(element),
        name: nameFor(element, actionable),
        children: [],
      };
      if (actionable) node.ref = referenceFor(element);
      if (typeof element.value === "string" && ["textarea", "select"].includes(tag)) {
        node.value = bounded(element.value);
      } else if (tag === "input" && typeof element.value === "string" &&
          !["password", "file"].includes(String(element.type || "").toLowerCase())) {
        node.value = bounded(element.value);
      }
      current.parent.push(node);

      const descendants = element.children || [];
      const available = Math.max(0, budget - visited - stack.length);
      const count = Math.min(descendants.length, available);
      if (count < descendants.length) truncated = true;
      for (let i = count - 1; i >= 0; i--) {
        stack.push({ element: descendants[i], parent: node.children, depth: current.depth + 1 });
      }
    }
    if (truncated) nodes.push({ role: "status", name: "[snapshot node limit reached]" });
    return { nodes, nodeCount: visited, truncated };
  }

  function resolve(reference) {
    const element = elements.get(reference);
    if (!element || element.isConnected === false) {
      elements.delete(reference);
      if (element) references.delete(element);
      throw new Error("stale or unknown automation reference");
    }
    return element;
  }

    Object.defineProperty(win, "__egoAutomation", {
      configurable: true,
      value: { localSnapshot, resolve },
    });
    return win.__egoAutomation;
  }

  install(window);
  Object.defineProperty(window, "__egoInstallAutomation", {
    configurable: true,
    value: install,
  });
})();
