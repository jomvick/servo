(() => {
    function roleFor(element) {
        if (element.getAttribute("role")) return element.getAttribute("role");
        const tag = element.localName;
        if (tag === "a" && element.hasAttribute("href")) return "link";
        if (tag === "button") return "button";
        if (tag === "textarea") return "textbox";
        if (tag === "select") return "combobox";
        if (tag === "input") {
            const type = (element.getAttribute("type") || "text").toLowerCase();
            if (type === "submit" || type === "button") return "button";
            if (type === "checkbox") return "checkbox";
            if (type === "radio") return "radio";
            return "textbox";
        }
        if (/^h[1-6]$/.test(tag)) return "heading";
        if (tag === "img") return "img";
        if (tag === "form") return "form";
        if (tag === "iframe") return "iframe";
        return null;
    }

    function accessibleName(element) {
        const labelledBy = element.getAttribute("aria-labelledby");
        if (labelledBy) {
            const labels = labelledBy.split(/\s+/)
                .map(id => element.ownerDocument.getElementById(id)?.textContent || "")
                .filter(Boolean);
            if (labels.length) return labels.join(" ");
        }
        const label = element.getAttribute("aria-label");
        if (label) return label;
        if (element.localName === "img") return element.getAttribute("alt") || "";
        if (element.localName === "input" || element.localName === "textarea") {
            const value = element.value;
            if (value) return value;
        }
        return (element.innerText || element.textContent || element.title || "").trim();
    }

    function visitFrame(win, path, depth) {
        const doc = win.document;
        const frame = { path, depth, nodes: [] };
        const childrenFrames = [];
        let nextRef = 0;
        let nextFrame = 0;

        function visit(element) {
            if (element.localName === "script" || element.localName === "style" ||
                element.localName === "noscript" || element.hidden ||
                element.getAttribute("aria-hidden") === "true") return null;

            if (element.localName === "iframe") {
                const node = {
                    role: "iframe",
                    name: element.title || element.src || "embedded content",
                };
                try {
                    const childWin = element.contentWindow;
                    if (!childWin || !childWin.document) throw new Error("unreadable");
                    childrenFrames.push(...visitFrame(childWin, `${path}.${nextFrame++}`, depth + 1));
                } catch (_) {
                    nextFrame++;
                    node.unreadable = true;
                }
                return node;
            }

            const role = roleFor(element);
            const isInteractive = role !== null &&
                (["button", "link", "textbox", "checkbox", "radio", "combobox",
                  "switch", "slider", "menuitem"].includes(role) ||
                 element.hasAttribute("tabindex"));
            const children = [];
            for (const child of element.children) {
                const node = visit(child);
                if (node) children.push(node);
            }

            const name = role || !children.length ? accessibleName(element) : "";
            if (!role && !children.length && !name) return null;
            const node = {
                role: role || "text",
                name,
            };
            if (isInteractive) {
                node.ref = `e${nextRef++}`;
                element.setAttribute("data-ego-ref", node.ref);
            }
            if ((element.localName === "input" || element.localName === "textarea") &&
                element.value) node.value = element.value;
            if (children.length) node.children = children;
            return node;
        }

        for (const child of doc.body ? doc.body.children : []) {
            const node = visit(child);
            if (node) frame.nodes.push(node);
        }
        return [frame, ...childrenFrames];
    }

    return {
        title: document.title || "",
        url: location.href,
        frames: visitFrame(window, "f0", 0),
    };
})()
