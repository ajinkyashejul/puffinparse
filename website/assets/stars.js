/* Live GitHub star count for the header GitHub button. One unauthenticated request, cached for an
   hour. Below 1,000 stars, or when the API is unavailable, the button stays a plain "GitHub" link;
   from 1,000 it becomes "Star" plus the count. */
(function () {
  "use strict";
  var els = document.querySelectorAll("[data-gh-stars]");
  if (!els.length) return;
  var KEY = "puffinparse-gh-stars";
  var now = Date.now();
  function show(n) {
    if (typeof n !== "number" || n < 1000) return;
    var text = n >= 1000 ? (n / 1000).toFixed(n >= 10000 ? 0 : 1).replace(/\.0$/, "") + "k" : String(n);
    els.forEach(function (el) {
      el.textContent = text;
      el.hidden = false;
      var a = el.closest("a");
      var label = a && a.querySelector("[data-gh-label]");
      if (label) label.textContent = "Star";
      if (a) a.setAttribute("aria-label", "Star PuffinParse on GitHub (" + n + " stars)");
    });
  }
  try {
    var cached = JSON.parse(localStorage.getItem(KEY) || "null");
    if (cached && now - cached.t < 3600000) return show(cached.n);
  } catch (e) {}
  fetch("https://api.github.com/repos/ajinkyashejul/puffinparse", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (d) {
      if (!d) return;
      show(d.stargazers_count);
      try { localStorage.setItem(KEY, JSON.stringify({ n: d.stargazers_count, t: now })); } catch (e) {}
    })
    .catch(function () {});
})();
