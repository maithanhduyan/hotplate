// ⚡ Hotplate — Live Reload Client Script
// Injected before </body> by the inject middleware.
//
// Server → Browser:
//   - "reload"      → full page reload
//   - "css:<path>"  → hot-swap only that stylesheet
//
// Browser → Server (JSON):
//   - {kind:"connect",url,ua,vw,vh}                — client identity on connect
//   - {kind:"js_error",msg,src,line,col,stack}      — runtime JS errors
//   - {kind:"console",level,msg}                    — console.warn/error
//   - {kind:"net_error",url,method,status,error}    — failed fetch requests
//
// Auto-reconnects after 1s on disconnect.

(() => {
  const p = location.protocol === "https:" ? "wss:" : "ws:";
  let t, ws;
  function send(obj) {
    try {
      if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj));
    } catch (_) { }
  }
  function reloadCSS(path) {
    const links = document.querySelectorAll('link[rel="stylesheet"]');
    let found = false;
    links.forEach((link) => {
      const href = link.getAttribute("href");
      if (!href) return;
      const clean = href.split("?")[0];
      if (
        clean === path ||
        clean === "/" + path ||
        clean.endsWith("/" + path)
      ) {
        link.href = clean + "?_lr=" + Date.now();
        found = true;
      }
    });
    if (!found) location.reload();
  }
  // Intercept console.warn and console.error
  const _warn = console.warn,
    _err = console.error;
  console.warn = (...a) => {
    send({ kind: "console", level: "warn", msg: a.join(" ") });
    _warn.apply(console, a);
  };
  console.error = (...a) => {
    send({ kind: "console", level: "error", msg: a.join(" ") });
    _err.apply(console, a);
  };
  // Capture unhandled JS errors
  window.onerror = (msg, src, line, col, err) => {
    send({
      kind: "js_error",
      msg: String(msg),
      src: src || "",
      line: line || 0,
      col: col || 0,
      stack: (err && err.stack) || "",
    });
  };
  window.onunhandledrejection = (e) => {
    const r = e.reason;
    send({
      kind: "js_error",
      msg: String(r),
      src: "",
      line: 0,
      col: 0,
      stack: (r && r.stack) || "",
    });
  };
  // Wrap fetch to track network requests
  const _fetch = window.fetch;
  window.fetch = function () {
    const url = (arguments[0] && arguments[0].url) || String(arguments[0]);
    const method = (arguments[1] && arguments[1].method) || "GET";
    const t0 = performance.now();
    return _fetch
      .apply(this, arguments)
      .then((r) => {
        const dur = Math.round(performance.now() - t0);
        send({
          kind: "net_request",
          url: url,
          method: method,
          status: r.status,
          duration: dur,
        });
        if (!r.ok)
          send({
            kind: "net_error",
            url: url,
            method: method,
            status: r.status,
            error: r.statusText,
          });
        return r;
      })
      .catch((e) => {
        const dur = Math.round(performance.now() - t0);
        send({
          kind: "net_request",
          url: url,
          method: method,
          status: 0,
          duration: dur,
        });
        send({
          kind: "net_error",
          url: url,
          method: method,
          status: 0,
          error: e.message,
        });
        throw e;
      });
  };
  function connect() {
    ws = new WebSocket(`${p}//${location.host}/__lr`);
    ws.onopen = () => {
      send({
        kind: "connect",
        url: location.href,
        ua: navigator.userAgent,
        vw: innerWidth,
        vh: innerHeight,
      });
    };
    ws.onmessage = (e) => {
      const d = e.data;
      if (d === "reload") location.reload();
      else if (d.startsWith("css:")) reloadCSS(d.slice(4));
      else if (d.startsWith("inject:js:")) {
        try {
          const s = document.createElement("script");
          s.textContent = d.slice(10);
          document.head.appendChild(s);
        } catch (err) { console.error("[hotplate inject]", err); }
      } else if (d.startsWith("inject:css:")) {
        try {
          const s = document.createElement("style");
          s.textContent = d.slice(11);
          document.head.appendChild(s);
        } catch (err) { console.error("[hotplate inject]", err); }
      } else if (d.startsWith("screenshot:")) {
        // screenshot:{id}:{width}x{height}
        const parts = d.slice(11).split(":");
        const id = parts[0];
        const dims = (parts[1] || "").split("x");
        const w = parseInt(dims[0]) || innerWidth;
        const h = parseInt(dims[1]) || innerHeight;
        (async () => {
          try {
            const c = document.createElement("canvas");
            c.width = w; c.height = h;
            const ctx = c.getContext("2d");
            ctx.fillStyle = "#fff";
            ctx.fillRect(0, 0, w, h);
            // Use html-to-image approach: serialize DOM to SVG foreignObject
            const html = document.documentElement.outerHTML;
            const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><foreignObject width="100%" height="100%"><div xmlns="http://www.w3.org/1999/xhtml">${html}</div></foreignObject></svg>`;
            const blob = new Blob([svg], {type: "image/svg+xml;charset=utf-8"});
            const url = URL.createObjectURL(blob);
            const img = new Image();
            img.onload = () => {
              ctx.drawImage(img, 0, 0, w, h);
              URL.revokeObjectURL(url);
              const base64 = c.toDataURL("image/png").split(",")[1];
              send({kind:"screenshot_response", url: id, msg: base64});
            };
            img.onerror = () => {
              URL.revokeObjectURL(url);
              send({kind:"screenshot_response", url: id, msg: ""});
            };
            img.src = url;
          } catch (_) {
            send({kind:"screenshot_response", url: id, msg: ""});
          }
        })();
      } else if (d.startsWith("dom_query:")) {
        // dom_query:{id}:{selector}
        const idx = d.indexOf(":", 10);
        const id = d.slice(10, idx);
        const selector = d.slice(idx + 1);
        try {
          const els = document.querySelectorAll(selector);
          const result = [];
          els.forEach((el, i) => {
            if (i >= 200) return; // cap at 200 elements
            const attrs = {};
            for (const a of el.attributes) attrs[a.name] = a.value;
            result.push({
              tag: el.tagName.toLowerCase(),
              text: (el.textContent || "").slice(0, 500),
              attributes: attrs,
              innerHTML: (el.innerHTML || "").slice(0, 1000)
            });
          });
          send({kind:"dom_response", url: id, msg: JSON.stringify(result)});
        } catch (err) {
          send({kind:"dom_response", url: id, msg: JSON.stringify({error: err.message})});
        }
      } else if (d.startsWith("eval:")) {
        // eval:{id}:{code}
        const idx = d.indexOf(":", 5);
        const id = d.slice(5, idx);
        const code = d.slice(idx + 1);
        (async () => {
          try {
            const fn = new Function("return (async () => {" + code + "})()");
            const result = await fn();
            let serialized;
            try {
              serialized = JSON.stringify(result);
            } catch (_) {
              serialized = String(result);
            }
            send({kind:"eval_response", url: id, msg: serialized || "undefined"});
          } catch (err) {
            send({kind:"eval_response", url: id, msg: JSON.stringify({error: err.message, stack: err.stack || ""})});
          }
        })();
      }
    };
    ws.onclose = () => {
      clearTimeout(t);
      t = setTimeout(connect, 1000);
    };
    ws.onerror = () => ws.close();
  }
  connect();
})();
