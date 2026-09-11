/* LiteOCR docs — small progressive enhancements. No dependencies. */
(function () {
  "use strict";
  var BASE = document.documentElement.getAttribute("data-base") || "/";

  /* ---- theme toggle (bootstrapped inline in <head> to avoid a flash) ---- */
  var toggle = document.getElementById("theme");
  if (toggle) {
    toggle.addEventListener("click", function () {
      var dark = document.documentElement.getAttribute("data-theme") === "dark" ||
        (!document.documentElement.getAttribute("data-theme") &&
          window.matchMedia("(prefers-color-scheme: dark)").matches);
      var next = dark ? "light" : "dark";
      document.documentElement.setAttribute("data-theme", next);
      try { localStorage.setItem("liteocr-theme", next); } catch (e) { /* private mode */ }
      var label = "Switch to " + (next === "dark" ? "light" : "dark") + " theme";
      toggle.setAttribute("aria-label", label);
      toggle.setAttribute("title", label);
    });
  }

  /* ---- copy button on every code block ---- */
  document.querySelectorAll("article pre > code").forEach(function (code) {
    var pre = code.parentNode;
    var wrap = document.createElement("div");
    wrap.className = "codewrap";
    pre.parentNode.insertBefore(wrap, pre);
    wrap.appendChild(pre);
    var btn = document.createElement("button");
    btn.className = "copy";
    btn.type = "button";
    btn.textContent = "Copy";
    btn.addEventListener("click", function () {
      var done = function () {
        btn.textContent = "Copied";
        btn.classList.add("done");
        setTimeout(function () { btn.textContent = "Copy"; btn.classList.remove("done"); }, 1400);
      };
      var fallback = function () {          // clipboard API is unavailable over plain http
        var ta = document.createElement("textarea");
        ta.value = code.textContent;
        ta.setAttribute("readonly", "");
        ta.style.cssText = "position:fixed;top:0;left:-9999px";
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand("copy"); } catch (e) { /* nothing else to try */ }
        document.body.removeChild(ta);
        done();
      };
      if (navigator.clipboard) navigator.clipboard.writeText(code.textContent).then(done, fallback);
      else fallback();
    });
    wrap.appendChild(btn);
  });

  /* ---- animated model swap in the hero ---- */
  var swap = document.getElementById("swap");
  if (swap && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    var models = JSON.parse(swap.getAttribute("data-models") || "[]");
    var i = 0;
    setInterval(function () {
      swap.classList.add("fade");
      setTimeout(function () {
        i = (i + 1) % models.length;
        swap.textContent = models[i];
        swap.classList.remove("fade");
      }, 190);
    }, 2200);
  }

  /* ---- "On this page" scroll highlighting ---- */
  var links = [].slice.call(document.querySelectorAll(".toc a[href^='#']"));
  if (links.length && "IntersectionObserver" in window) {
    var byId = {};
    links.forEach(function (a) { byId[decodeURIComponent(a.hash.slice(1))] = a; });
    var seen = [];
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        var id = e.target.id;
        var at = seen.indexOf(id);
        if (e.isIntersecting && at < 0) seen.push(id);
        if (!e.isIntersecting && at >= 0) seen.splice(at, 1);
      });
      var active = seen.length ? seen[0] : null;
      links.forEach(function (a) { a.classList.remove("on"); });
      if (active && byId[active]) byId[active].classList.add("on");
    }, { rootMargin: "-70px 0px -70% 0px" });
    Object.keys(byId).forEach(function (id) {
      var el = document.getElementById(id);
      if (el) io.observe(el);
    });
  }

  /* ---- client-side search over page titles + headings ---- */
  var input = document.getElementById("search");
  var panel = document.getElementById("results");
  if (!input || !panel) return;
  var index = null, rows = [], cursor = -1;

  function load() {
    if (index) return Promise.resolve(index);
    return fetch(BASE + "search.json").then(function (r) { return r.json(); }).then(function (d) {
      index = [];
      d.forEach(function (p) {
        index.push({ t: p.t, s: p.t, u: p.u, page: null });
        (p.h || []).forEach(function (h) { index.push({ t: h[0], s: p.t, u: p.u + "#" + h[1], page: p.t }); });
      });
      return index;
    }).catch(function () { index = []; return index; });
  }

  function render(q) {
    var needle = q.trim().toLowerCase();
    rows = [];
    if (needle && index) {
      rows = index.filter(function (r) { return r.t.toLowerCase().indexOf(needle) >= 0; })
        .sort(function (a, b) { return a.t.toLowerCase().indexOf(needle) - b.t.toLowerCase().indexOf(needle); })
        .slice(0, 12);
    }
    panel.innerHTML = "";
    cursor = rows.length ? 0 : -1;
    if (!rows.length) { panel.hidden = true; return; }
    rows.forEach(function (r, n) {
      var a = document.createElement("a");
      a.href = r.u;
      a.textContent = r.t;
      if (r.page) { var s = document.createElement("small"); s.textContent = r.page; a.appendChild(s); }
      if (n === 0) a.className = "on";
      panel.appendChild(a);
    });
    panel.hidden = false;
  }

  function move(step) {
    var items = panel.querySelectorAll("a");
    if (!items.length) return;
    items[cursor] && items[cursor].classList.remove("on");
    cursor = (cursor + step + items.length) % items.length;
    items[cursor].classList.add("on");
    items[cursor].scrollIntoView({ block: "nearest" });
  }

  input.addEventListener("focus", function () { load().then(function () { render(input.value); }); });
  input.addEventListener("input", function () { load().then(function () { render(input.value); }); });
  input.addEventListener("keydown", function (e) {
    if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
    else if (e.key === "Enter") { var a = panel.querySelector("a.on"); if (a) { e.preventDefault(); location.href = a.href; } }
    else if (e.key === "Escape") { input.blur(); panel.hidden = true; }
  });
  document.addEventListener("click", function (e) { if (!panel.contains(e.target) && e.target !== input) panel.hidden = true; });
  document.addEventListener("keydown", function (e) {
    var typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName);
    if ((e.key === "/" && !typing) || (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey))) {
      e.preventDefault();
      input.focus();
      input.select();
    }
  });
})();
