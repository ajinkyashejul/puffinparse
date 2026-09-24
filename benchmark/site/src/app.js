/* LiteOCR benchmark results viewer.
 *
 * Vanilla ES2018, no framework, no build step (ADR: the viewer stays vanilla). Everything is
 * fetched from the `data/` directory written by benchmark/site/build.py, through `url()`; the
 * prefix comes from `<meta name="liteocr-base">`. The only third-party code is pdf.js, loaded
 * lazily from cdnjs (pinned version + SRI) the first time a PDF input is opened; if it cannot
 * load, the viewer falls back to the build-time page-1 PNG preview.
 *
 * Routes (hash based, so every view is a shareable link and no rewrite rules are needed):
 *   #/leaderboard?run=<run_id>&sort=<col>&dir=asc|desc&x=cost|latency
 *   #/documents?run=<run_id>&src=<source>&cat=<category>&q=<text>&sort=<col>&dir=asc|desc
 *   #/<run_id>/<model_slug>/<doc_id…>?tab=diff|compare|truth|rules|output&diff=split|unified
 *                                      &ov=0|1&page=<n>&src=…&cat=…&q=…
 *   #/doc/<doc_id>?run=…&model=…   (old links; redirected to the canonical form above)
 */
(function () {
  "use strict";

  var REPO = "https://github.com/ajinkyashejul/liteocr";
  var METHODOLOGY = REPO + "/blob/main/benchmark/README.md";
  var DIFF_CELL_CAP = 6000000; // LCS table cells we are willing to allocate
  var RULE_PAGE = 150; // checklist items rendered per "show more"
  var BAG_SENTENCE_CAP = 12; // sentences listed per bag_of_sentences rule before "+N more"

  // pdf.js is the one external dependency: pinned, integrity-checked, loaded on demand.
  var PDFJS = {
    lib: "https://cdnjs.cloudflare.com/ajax/libs/pdf.js/3.11.174/pdf.min.js",
    libSri: "sha384-/1qUCSGwTur9vjf/z9lmu/eCUYbpOTgSjmpbMQZ1/CtX2v/WcAIKqRv+U1DUCG6e",
    worker: "https://cdnjs.cloudflare.com/ajax/libs/pdf.js/3.11.174/pdf.worker.min.js",
    workerSri: "sha384-SnzOobpRMLXZ52iJvZm/C0fYw0OQemTXzTjIsdsfMcrCtCEe9qgzxTd3RSklO5x2",
  };

  // Unified block types folded into eight groups: one categorical hue each (fixed order).
  var BLOCK_GROUPS = [
    { key: "text", label: "Text", types: ["text"] },
    { key: "heading", label: "Title / heading", types: ["title", "section_header"] },
    { key: "table", label: "Table", types: ["table"] },
    { key: "figure", label: "Figure", types: ["figure"] },
    { key: "list", label: "List", types: ["list"] },
    { key: "furniture", label: "Header / footer", types: ["header", "footer", "footnote"] },
    { key: "caption", label: "Caption", types: ["caption"] },
    { key: "other", label: "Formula / other", types: ["formula", "other"] },
  ];

  function meta(name, fallback) {
    var el = document.querySelector('meta[name="' + name + '"]');
    var value = el && el.getAttribute("content");
    return value || fallback;
  }

  var BASE = (function () {
    var value = meta("liteocr-base", "./");
    return value.charAt(value.length - 1) === "/" ? value : value + "/";
  })();
  var HOME = meta("liteocr-home", "");
  var DOCS = meta("liteocr-docs", "");

  var INDEX = null;
  var runCache = {};
  var textCache = {};
  var jsonCache = {};
  var manifestCache = {};
  var lastScrollKey = null;
  var KEYS = {}; // key handlers of the current view
  var SV = null; // the persistent source viewer (kept across model / tab switches)

  var view = document.getElementById("view");

  function url(path) {
    return BASE + path;
  }

  /* ------------------------------------------------------------------ utils */

  function esc(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function isNum(value) {
    return typeof value === "number" && isFinite(value);
  }

  function fixed(value, digits) {
    return isNum(value) ? value.toFixed(digits) : "—";
  }

  function msFmt(value) {
    if (!isNum(value)) return "—";
    return value >= 10000 ? (value / 1000).toFixed(1) + " s" : Math.round(value) + " ms";
  }

  function moneyFmt(value) {
    return isNum(value) ? "$" + value.toFixed(2) : "—";
  }

  function slugOf(model) {
    return String(model).replace(/\//g, "_");
  }

  function enc(segment) {
    return encodeURIComponent(segment);
  }

  function encPath(id) {
    return String(id).split("/").map(enc).join("/");
  }

  /** Join and collapse `.`/`..` segments, so combined-dataset paths stay readable. */
  function joinPath(base, rel) {
    var parts = (String(base) + String(rel == null ? "" : rel)).split("/");
    var out = [];
    for (var i = 0; i < parts.length; i++) {
      var part = parts[i];
      if (part === ".") continue;
      if (part === ".." && out.length > 1 && out[out.length - 1] !== "..") {
        out.pop();
        continue;
      }
      out.push(part);
    }
    return out.join("/");
  }

  function pathLabel(rel) {
    return String(rel == null ? "" : rel).replace(/^(?:\.\.\/)+/, "");
  }

  function isRulesDoc(entry) {
    return !!entry && (entry.kind === "rules" || (!entry.truth && !!entry.rules));
  }

  function fetchOk(target) {
    return fetch(target, { cache: "no-cache" }).then(function (resp) {
      if (!resp.ok) throw new Error("Could not load " + target + " (HTTP " + resp.status + ")");
      return resp;
    });
  }

  function getJSON(target) {
    if (jsonCache[target]) return jsonCache[target];
    var promise = fetchOk(target).then(function (resp) {
      return resp.json();
    });
    jsonCache[target] = promise;
    promise.catch(function () {
      delete jsonCache[target];
    });
    return promise;
  }

  function getText(target) {
    if (textCache[target]) return textCache[target];
    var promise = fetchOk(target).then(function (resp) {
      return resp.text();
    });
    textCache[target] = promise;
    promise.catch(function () {
      delete textCache[target];
    });
    return promise;
  }

  function getRun(runId) {
    if (runCache[runId]) return Promise.resolve(runCache[runId]);
    return getJSON(url("data/runs/" + enc(runId) + ".json")).then(function (data) {
      analyse(data);
      runCache[runId] = data;
      return data;
    });
  }

  function getManifest(name) {
    if (manifestCache[name]) return Promise.resolve(manifestCache[name]);
    return getJSON(url("data/datasets/" + enc(name) + "/manifest.json")).then(function (data) {
      manifestCache[name] = data;
      return data;
    });
  }

  /** Assertion file of a `kind: "rules"` document. null means "cannot show". */
  function getRules(rulesUrl) {
    return getJSON(rulesUrl).then(
      function (data) {
        return Array.isArray(data) ? data : (data && data.rules) || null;
      },
      function () {
        return null;
      }
    );
  }

  /* --------------------------------------------------------------- scoring */

  /** The number a model is ranked by: `summary.headline` when present, else `overall`. */
  function headlineOf(summary) {
    summary = summary || {};
    var head = summary.headline;
    var value = head;
    var label = null;
    var title = null;
    if (head && typeof head === "object") {
      value = [head.score, head.overall, head.value].filter(isNum)[0];
      label = head.label || head.name || head.metric || null;
      title = head.description || head.title || null;
    }
    if (isNum(value)) {
      if (value <= 1 && isNum(summary.overall) && summary.overall > 1) value *= 100;
      return { value: value, headline: true, label: label || "Headline", title: title };
    }
    return { value: summary.overall, headline: false, label: "Overall", title: null };
  }

  /** Per-document primary score on the 0–100 scale, as the scorer aggregates it. */
  function docScore(rec) {
    if (!rec) return null;
    if (rec.error) return 0;
    if (isNum(rec.headline)) return rec.headline <= 1 ? rec.headline * 100 : rec.headline;
    var m = rec.metrics || {};
    if (rec.table_only && isNum(m.table_score)) return m.table_score * 100;
    return isNum(m.char_similarity) ? m.char_similarity * 100 : null;
  }

  function scoreBasis(rec) {
    if (!rec) return "";
    var m = rec.metrics || {};
    if (isNum(m.rule_pass_rate)) return "rule pass rate";
    if (rec.table_only && isNum(m.table_score)) return "table score (table-only document)";
    return "character similarity";
  }

  function sourceOf(docId, fallback) {
    var cut = String(docId).indexOf("/");
    return cut > 0 ? String(docId).slice(0, cut) : fallback;
  }

  /** Nearest-rank percentile, identical to `percentile` in crates/liteocr-cli/src/bench.rs. */
  function percentile(sorted, p) {
    if (!sorted.length) return null;
    var idx = Math.round((sorted.length - 1) * p);
    return sorted[Math.min(idx, sorted.length - 1)];
  }

  function bucket() {
    return { sum: 0, docs: 0, failed: 0 };
  }

  function addTo(target, score, rec) {
    target.docs += 1;
    if (rec.error) target.failed += 1;
    target.sum += isNum(score) ? score : 0;
  }

  /** Derived numbers the result file does not carry: p90, per-source and source×category. */
  function analyse(run) {
    var fallback = (run.dataset && run.dataset.name) || "dataset";
    var sources = [];
    var catsBySource = {};
    (run.models || []).forEach(function (model) {
      var lat = [];
      var bySource = {};
      var byCat = {};
      (model.docs || []).forEach(function (rec) {
        var src = sourceOf(rec.id, fallback);
        var cat = rec.category || "?";
        if (sources.indexOf(src) === -1) sources.push(src);
        catsBySource[src] = catsBySource[src] || [];
        if (catsBySource[src].indexOf(cat) === -1) catsBySource[src].push(cat);
        var score = docScore(rec);
        addTo((bySource[src] = bySource[src] || bucket()), score, rec);
        var key = src + "\u0000" + cat;
        addTo((byCat[key] = byCat[key] || bucket()), score, rec);
        if (!rec.error && isNum(rec.latency_ms)) lat.push(rec.latency_ms);
      });
      lat.sort(function (a, b) {
        return a - b;
      });
      var summary = model.summary || {};
      model.stats = {
        p50: isNum(summary.latency_p50_ms) ? summary.latency_p50_ms : percentile(lat, 0.5),
        p90: isNum(summary.latency_p90_ms) ? summary.latency_p90_ms : percentile(lat, 0.9),
        p95: isNum(summary.latency_p95_ms) ? summary.latency_p95_ms : percentile(lat, 0.95),
        head: headlineOf(summary),
        bySource: bySource,
        byCat: byCat,
      };
    });
    Object.keys(catsBySource).forEach(function (src) {
      catsBySource[src].sort();
    });
    run.sources = sources;
    run.catsBySource = catsBySource;
  }

  function mean(b) {
    return b && b.docs ? b.sum / b.docs : null;
  }

  /* ----------------------------------------------------------------- router */

  function parseHash() {
    var raw = location.hash.replace(/^#/, "") || "/leaderboard";
    var split = raw.indexOf("?");
    var pathPart = split === -1 ? raw : raw.slice(0, split);
    var queryPart = split === -1 ? "" : raw.slice(split + 1);
    var parts = pathPart.split("/").filter(Boolean).map(function (p) {
      try {
        return decodeURIComponent(p);
      } catch (e) {
        return p;
      }
    });
    return { parts: parts, params: new URLSearchParams(queryPart) };
  }

  function query(values) {
    var q = new URLSearchParams();
    Object.keys(values || {}).forEach(function (key) {
      var value = values[key];
      if (value !== null && value !== undefined && value !== "") q.set(key, value);
    });
    var s = q.toString();
    return s ? "?" + s : "";
  }

  function listHref(path, runId, values) {
    var all = { run: runId };
    Object.keys(values || {}).forEach(function (k) {
      all[k] = values[k];
    });
    return "#/" + path + query(all);
  }

  function docHref(runId, model, docId, values) {
    return "#/" + enc(runId) + "/" + enc(slugOf(model)) + "/" + encPath(docId) + query(values);
  }

  function go(hash, replace) {
    if (replace) {
      history.replaceState(null, "", hash);
      render();
    } else {
      location.hash = hash;
    }
  }

  function findRun(runId) {
    var runs = (INDEX && INDEX.runs) || [];
    for (var i = 0; i < runs.length; i++) if (runs[i].run_id === runId) return runs[i];
    return null;
  }

  function resolveRun(params) {
    return findRun(params.get("run")) || (INDEX.runs || [])[0];
  }

  function setNav(active, runId) {
    ["leaderboard", "documents"].forEach(function (name) {
      var link = document.getElementById("nav-" + name);
      if (!link) return;
      link.setAttribute("href", listHref(name, runId));
      if (name === active) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }

  function showError(err) {
    view.removeAttribute("aria-busy");
    view.innerHTML =
      '<div class="error"><h2>Something went wrong</h2><p>' +
      esc(err && err.message ? err.message : err) +
      '</p><p class="hint">This page is static: it reads the JSON and markdown written by ' +
      "<code>benchmark/site/build.py</code>. Serve <code>dist/</code> over HTTP (for example " +
      "<code>python -m http.server -d benchmark/site/dist 8000</code>) rather than opening the " +
      "file directly.</p></div>";
  }

  /* ------------------------------------------------------- shared fragments */

  function runSelector(run) {
    var options = (INDEX.runs || [])
      .map(function (item) {
        var label =
          (item.dataset.name || "?") +
          " v" +
          (item.dataset.version || "?") +
          " · " +
          (item.created_at || "").slice(0, 10) +
          " · " +
          item.run_id;
        return (
          '<option value="' +
          esc(item.run_id) +
          '"' +
          (item.run_id === run.run_id ? " selected" : "") +
          ">" +
          esc(label) +
          "</option>"
        );
      })
      .join("");
    return (
      '<div class="field"><label for="run-select">Run</label><select id="run-select">' +
      options +
      "</select></div>"
    );
  }

  function wireRunSelect(root, path) {
    var select = root.querySelector("#run-select");
    if (!select) return;
    select.addEventListener("change", function () {
      go(listHref(path, select.value));
    });
  }

  function runMeta(run) {
    var sha = ((run.dataset && run.dataset.sha256) || "").slice(0, 12);
    var normalize = run.normalize || {};
    var flags = [];
    if (normalize.case_insensitive) flags.push("case-insensitive");
    if (normalize.strip_markdown) flags.push("markdown stripped");
    if (normalize.strip_punctuation) flags.push("punctuation stripped");
    return (
      '<p class="run-meta">' +
      "<span>Dataset <strong>" +
      esc(run.dataset.name) +
      "</strong> v" +
      esc(run.dataset.version) +
      "</span><span>" +
      esc(run.dataset.documents) +
      " documents</span><span>" +
      esc((run.models || []).length) +
      " models</span><span>LiteOCR " +
      esc(run.liteocr_version) +
      "</span>" +
      (run.scorer_version ? "<span>scorer " + esc(run.scorer_version) + "</span>" : "") +
      "<span>" +
      esc(run.created_at) +
      "</span>" +
      '<span title="SHA-256 of the manifest, every input, truth and rule file">sha256 <code>' +
      esc(sha) +
      "…</code></span>" +
      (flags.length ? "<span>" + esc(flags.join(", ")) + "</span>" : "") +
      "</p>"
    );
  }

  function codeBlock(command) {
    return (
      '<div class="code-block"><button class="copy-btn" type="button" data-copy="' +
      esc(command) +
      '">Copy</button><pre><code>' +
      esc(command) +
      "</code></pre></div>"
    );
  }

  function copyText(text, button, label) {
    var done = function () {
      button.textContent = "Copied";
      button.classList.add("done");
      setTimeout(function () {
        button.textContent = label;
        button.classList.remove("done");
      }, 1400);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, function () {
        button.textContent = "Press ⌘/Ctrl+C";
      });
      return;
    }
    var area = document.createElement("textarea");
    area.value = text;
    document.body.appendChild(area);
    area.select();
    try {
      document.execCommand("copy");
      done();
    } finally {
      document.body.removeChild(area);
    }
  }

  function wireCopyButtons(root) {
    root.querySelectorAll(".copy-btn").forEach(function (button) {
      button.addEventListener("click", function () {
        copyText(button.getAttribute("data-copy") || "", button, "Copy");
      });
    });
  }

  /** Generic sortable table. `columns` describe cells; `rows` are opaque records. */
  function sortableTable(options) {
    var columns = options.columns;
    var rows = options.rows.slice();
    var sortKey = options.sortKey;
    var sortDir = options.sortDir;
    var column = null;
    columns.forEach(function (candidate) {
      if (candidate.key === sortKey) column = candidate;
    });
    if (column && column.value) {
      var sign = sortDir === "asc" ? 1 : -1;
      rows.sort(function (a, b) {
        var av = column.value(a);
        var bv = column.value(b);
        if (av === null || av === undefined) return 1;
        if (bv === null || bv === undefined) return -1;
        if (typeof av === "string" || typeof bv === "string") {
          return String(av).localeCompare(String(bv)) * sign;
        }
        return (av - bv) * sign;
      });
    }

    var head = columns
      .map(function (col) {
        var isSorted = col.key === sortKey;
        var attrs =
          (col.num ? ' class="num"' : "") +
          (isSorted ? ' aria-sort="' + (sortDir === "asc" ? "ascending" : "descending") + '"' : "");
        if (!col.value) {
          return '<th scope="col"' + attrs + '><span class="sort-btn">' + esc(col.label) + "</span></th>";
        }
        return (
          '<th scope="col"' +
          attrs +
          '><button type="button" class="sort-btn" data-sort="' +
          esc(col.key) +
          '"' +
          (col.asc ? ' data-asc="1"' : "") +
          ' title="' +
          esc(col.title || "Sort by " + col.label) +
          '">' +
          esc(col.label) +
          '<span class="sort-arrow" aria-hidden="true">' +
          (isSorted ? (sortDir === "asc" ? "▲" : "▼") : "") +
          "</span></button></th>"
        );
      })
      .join("");

    var body = rows
      .map(function (row, position) {
        var cells = columns
          .map(function (col) {
            var cell = col.cell(row, position);
            var cls = ((col.num ? "num " : "") + (cell.cls || "")).trim();
            var tag = col.rowHeader ? "th" : "td";
            return (
              "<" +
              tag +
              (col.rowHeader ? ' scope="row"' : "") +
              (cls ? ' class="' + esc(cls) + '"' : "") +
              (cell.style ? ' style="' + esc(cell.style) + '"' : "") +
              (cell.title ? ' title="' + esc(cell.title) + '"' : "") +
              ">" +
              cell.html +
              "</" +
              tag +
              ">"
            );
          })
          .join("");
        var rowCls = options.rowClass ? options.rowClass(row, position) : "";
        return "<tr" + (rowCls ? ' class="' + esc(rowCls) + '"' : "") + ">" + cells + "</tr>";
      })
      .join("");

    return (
      '<div class="table-wrap"><table' +
      (options.id ? ' id="' + esc(options.id) + '"' : "") +
      ">" +
      (options.caption ? "<caption>" + options.caption + "</caption>" : "") +
      "<thead><tr>" +
      head +
      "</tr></thead><tbody>" +
      (body || '<tr><td colspan="' + columns.length + '" class="empty">No rows</td></tr>') +
      "</tbody></table></div>"
    );
  }

  function wireSorting(root, hrefFor) {
    root.querySelectorAll("button[data-sort]").forEach(function (button) {
      button.addEventListener("click", function () {
        var key = button.getAttribute("data-sort");
        var params = parseHash().params;
        var dir;
        if (params.get("sort") === key) dir = params.get("dir") === "asc" ? "desc" : "asc";
        else dir = button.getAttribute("data-asc") === "1" ? "asc" : "desc";
        go(hrefFor(key, dir));
      });
    });
  }

  /** Single-hue sequential scale (0..1) across a set of values; null for missing. */
  function heatScale(values) {
    var clean = values.filter(isNum);
    var lo = clean.length ? Math.min.apply(null, clean) : 0;
    var hi = clean.length ? Math.max.apply(null, clean) : 100;
    if (hi - lo < 5) lo = hi - 5; // do not amplify sub-point noise into a full gradient
    return function (value) {
      if (!isNum(value)) return null;
      return Math.max(0, Math.min(1, (value - lo) / (hi - lo || 1)));
    };
  }

  function heatStyle(t) {
    return t === null ? "" : "--t:" + t.toFixed(3);
  }

  /* ------------------------------------------------------------ leaderboard */

  var ASC_BY_DEFAULT = ["model", "cer", "wer", "p50", "p90", "p95", "per_page", "cost", "failed"];

  function leaderboardColumns(run, rulesRun, headLabel, headTitle) {
    function sum(key) {
      return function (row) {
        return row.summary[key];
      };
    }
    function numCol(key, label, digits, title, fmt, getter) {
      var get = getter || sum(key);
      return {
        key: key,
        label: label,
        num: true,
        title: title,
        asc: ASC_BY_DEFAULT.indexOf(key) !== -1,
        value: get,
        cell: function (row) {
          var v = get(row);
          return { html: esc(fmt ? fmt(v) : fixed(v, digits)) };
        },
      };
    }
    var cols = [
      {
        key: "rank",
        label: "#",
        cell: function (row, position) {
          return { html: String(position + 1), cls: "rank" };
        },
      },
      {
        key: "model",
        label: "Model",
        asc: true,
        rowHeader: true,
        value: function (row) {
          return row.model;
        },
        cell: function (row) {
          var first = (row.docs || [])[0];
          return {
            html:
              '<a class="model-link" href="' +
              esc(first ? docHref(run.run_id, row.model, first.id, {}) : "#") +
              '" title="Inspect this model document by document"><code>' +
              esc(row.model) +
              "</code></a>",
            cls: "model",
          };
        },
      },
      {
        key: "score",
        label: headLabel,
        num: true,
        title: headTitle,
        value: function (row) {
          return row.stats.head.value;
        },
        cell: function (row) {
          return { html: esc(fixed(row.stats.head.value, 2)), cls: "primary" };
        },
      },
      numCol("char_similarity", "Char sim", 3, "Mean primary per-document score (0–1)"),
      numCol("cer", "CER", 3, "Character error rate — lower is better"),
      numCol("wer", "WER", 3, "Word error rate — lower is better"),
      numCol("word_f1", "Word F1", 3),
      numCol("order_score", "Order", 3, "Reading-order agreement of shared lines"),
      numCol("table_score", "Table", 3, "Similarity restricted to markdown table rows"),
    ];
    if (rulesRun) {
      cols.push(
        numCol("rule_pass_rate", "Rules %", 1, "Mean assertion pass rate over rule-scored documents", function (v) {
          return isNum(v) ? (v * 100).toFixed(1) : "—";
        })
      );
    }
    cols.push(
      numCol("p50", "p50", 0, "Median client-measured latency", msFmt, function (row) {
        return row.stats.p50;
      }),
      numCol("p90", "p90", 0, "90th percentile latency (computed from the per-document latencies)", msFmt, function (row) {
        return row.stats.p90;
      }),
      numCol("p95", "p95", 0, "95th percentile latency", msFmt, function (row) {
        return row.stats.p95;
      }),
      numCol("per_page", "ms/page", 0, "Total latency / pages", null, sum("latency_per_page_ms")),
      numCol("cost", "$/1k pages", 2, "Public list price per 1,000 pages", moneyFmt, sum("cost_per_1k_pages_usd")),
      {
        key: "failed",
        label: "Failed",
        num: true,
        asc: true,
        value: sum("failed"),
        cell: function (row) {
          var failed = row.summary.failed || 0;
          return { html: esc(failed + "/" + row.summary.documents), cls: failed ? "fail" : "muted" };
        },
      }
    );
    return cols;
  }

  function breakdownTables(run, orderedModels) {
    var sources = run.sources || [];
    var values = [];
    orderedModels.forEach(function (model) {
      sources.forEach(function (src) {
        values.push(mean(model.stats.bySource[src]));
      });
    });
    var hue = heatScale(values);
    var srcHead = sources
      .map(function (src) {
        var n = (orderedModels[0] && orderedModels[0].stats.bySource[src]) || { docs: 0 };
        return (
          '<th scope="col" class="num"><a href="' +
          esc(listHref("documents", run.run_id, { src: src })) +
          '">' +
          esc(src) +
          '</a><span class="th-sub">' +
          n.docs +
          " docs</span></th>"
        );
      })
      .join("");
    var srcBody = orderedModels
      .map(function (model) {
        return (
          '<tr><th scope="row" class="model"><code>' +
          esc(model.model) +
          "</code></th>" +
          sources
            .map(function (src) {
              var b = model.stats.bySource[src];
              var v = mean(b);
              return (
                '<td class="num heat" style="' +
                heatStyle(hue(v)) +
                '" title="' +
                esc(model.model + " · " + src + " · " + (b ? b.docs : 0) + " docs" + (b && b.failed ? " · " + b.failed + " failed" : "")) +
                '">' +
                esc(fixed(v, 1)) +
                "</td>"
              );
            })
            .join("") +
          "</tr>"
        );
      })
      .join("");

    var catValues = [];
    var cols = [];
    sources.forEach(function (src) {
      (run.catsBySource[src] || []).forEach(function (cat) {
        cols.push({ src: src, cat: cat });
      });
    });
    orderedModels.forEach(function (model) {
      cols.forEach(function (c) {
        catValues.push(mean(model.stats.byCat[c.src + "\u0000" + c.cat]));
      });
    });
    var catHue = heatScale(catValues);
    var groupHead = sources
      .map(function (src) {
        return (
          '<th scope="colgroup" class="group" colspan="' +
          (run.catsBySource[src] || []).length +
          '">' +
          esc(src) +
          "</th>"
        );
      })
      .join("");
    var catHead = cols
      .map(function (c) {
        return (
          '<th scope="col" class="num"><a href="' +
          esc(listHref("documents", run.run_id, { src: c.src, cat: c.cat })) +
          '">' +
          esc(c.cat) +
          "</a></th>"
        );
      })
      .join("");
    var catBody = orderedModels
      .map(function (model) {
        return (
          '<tr><th scope="row" class="model"><code>' +
          esc(model.model) +
          "</code></th>" +
          cols
            .map(function (c) {
              var b = model.stats.byCat[c.src + "\u0000" + c.cat];
              var v = mean(b);
              return (
                '<td class="num heat" style="' +
                heatStyle(catHue(v)) +
                '" title="' +
                esc(model.model + " · " + c.src + " / " + c.cat + " · " + (b ? b.docs : 0) + " docs") +
                '">' +
                esc(fixed(v, 1)) +
                "</td>"
              );
            })
            .join("") +
          "</tr>"
        );
      })
      .join("");

    return (
      '<section class="block"><div class="panel-title"><h2>By source</h2>' +
      '<span class="heat-legend">lower <span class="heat-bar"></span> higher</span></div>' +
      '<div class="table-wrap"><table class="compact"><caption>Mean per-document score per dataset source. ' +
      "Sources are scored on their own ground truth, so compare models within a column, not across columns.</caption>" +
      '<thead><tr><th scope="col">Model</th>' +
      srcHead +
      "</tr></thead><tbody>" +
      srcBody +
      "</tbody></table></div></section>" +
      '<section class="block"><div class="panel-title"><h2>By category</h2>' +
      '<span class="hint">click a heading to list its documents</span></div>' +
      '<div class="table-wrap"><table class="compact heatmap"><caption>Mean per-document score per source and category. ' +
      "Shading is scaled across the whole table.</caption>" +
      '<thead><tr><td></td>' +
      groupHead +
      '</tr><tr><th scope="col">Model</th>' +
      catHead +
      "</tr></thead><tbody>" +
      catBody +
      "</tbody></table></div></section>"
    );
  }

  /* ---------------------------------------------------------------- scatter */

  function niceTicks(lo, hi, count) {
    var span = hi - lo || 1;
    var step = Math.pow(10, Math.floor(Math.log10(span / count)));
    var err = span / count / step;
    if (err >= 7.5) step *= 10;
    else if (err >= 3.5) step *= 5;
    else if (err >= 1.5) step *= 2;
    var ticks = [];
    for (var v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) ticks.push(+v.toFixed(10));
    return ticks;
  }

  function scatterChart(run, models, xMode) {
    var xLabel = xMode === "latency" ? "p50 latency (s)" : "List price, $ per 1,000 pages";
    var points = models
      .map(function (m) {
        var x = xMode === "latency" ? (isNum(m.stats.p50) ? m.stats.p50 / 1000 : null) : m.summary.cost_per_1k_pages_usd;
        return { model: m, x: x, y: m.stats.head.value };
      })
      .filter(function (p) {
        return isNum(p.x) && isNum(p.y);
      });
    if (!points.length) return '<p class="empty">No cost data in this run.</p>';

    var W = 720;
    var H = 340;
    var M = { l: 52, r: 24, t: 16, b: 46 };
    var xMax = Math.max.apply(null, points.map(function (p) { return p.x; }));
    var yMin = Math.min.apply(null, points.map(function (p) { return p.y; }));
    var yMax = Math.max.apply(null, points.map(function (p) { return p.y; }));
    var xTicks = niceTicks(0, xMax * 1.12 || 1, 5);
    var x1 = xTicks[xTicks.length - 1] || 1;
    var y0 = Math.max(0, Math.floor((yMin - Math.max(2, (yMax - yMin) * 0.15)) / 5) * 5);
    var y1 = Math.min(100, Math.ceil((yMax + Math.max(1, (yMax - yMin) * 0.1)) / 5) * 5);
    if (y1 <= y0) y1 = y0 + 5;
    var yTicks = niceTicks(y0, y1, 5);
    function sx(v) {
      return M.l + (v / x1) * (W - M.l - M.r);
    }
    function sy(v) {
      return H - M.b - ((v - y0) / (y1 - y0)) * (H - M.t - M.b);
    }

    // Pareto frontier: no other model is both cheaper (or faster) and better.
    var sorted = points.slice().sort(function (a, b) {
      return a.x - b.x || b.y - a.y;
    });
    var best = -Infinity;
    sorted.forEach(function (p) {
      p.front = p.y > best;
      if (p.front) best = p.y;
    });
    var front = sorted.filter(function (p) {
      return p.front;
    });
    var frontPath = front
      .map(function (p, i) {
        // Step line: flat until the next frontier model, then up to it.
        return i === 0 ? "M" + sx(p.x) + "," + sy(p.y) : "H" + sx(p.x) + "V" + sy(p.y);
      })
      .join("");

    // Direct labels, nudged apart vertically when they would collide.
    var labels = points
      .map(function (p) {
        return { p: p, x: sx(p.x), y: sy(p.y) };
      })
      .sort(function (a, b) {
        return a.y - b.y;
      });
    var placed = [];
    labels.forEach(function (l) {
      var ly = l.y + 4;
      var right = l.x < W - 190;
      placed.forEach(function (o) {
        if (Math.abs(o.x - l.x) < 150 && o.right === right && Math.abs(o.ly - ly) < 14) ly = o.ly + 14;
      });
      l.ly = ly;
      l.right = right;
      placed.push(l);
    });

    var grid =
      yTicks
        .map(function (t) {
          return (
            '<line class="grid" x1="' + M.l + '" x2="' + (W - M.r) + '" y1="' + sy(t) + '" y2="' + sy(t) + '"/>' +
            '<text class="tick" x="' + (M.l - 8) + '" y="' + (sy(t) + 4) + '" text-anchor="end">' + t + "</text>"
          );
        })
        .join("") +
      xTicks
        .map(function (t) {
          return (
            '<text class="tick" x="' + sx(t) + '" y="' + (H - M.b + 18) + '" text-anchor="middle">' +
            (xMode === "latency" ? t : "$" + t) +
            "</text>"
          );
        })
        .join("");

    var marks = labels
      .map(function (l) {
        var p = l.p;
        var tip =
          p.model.model +
          " — " +
          p.model.stats.head.label.toLowerCase() +
          " " +
          fixed(p.y, 2) +
          " · $" +
          fixed(p.model.summary.cost_per_1k_pages_usd, 2) +
          "/1k pages · p50 " +
          msFmt(p.model.stats.p50) +
          (p.front ? " · on the frontier" : "");
        return (
          '<g class="pt' +
          (p.front ? " front" : "") +
          '" tabindex="0" role="img" aria-label="' +
          esc(tip) +
          '" data-tip="' +
          esc(tip) +
          '">' +
          '<circle class="hit" cx="' + l.x + '" cy="' + l.y + '" r="14"/>' +
          '<circle class="dot" cx="' + l.x + '" cy="' + l.y + '" r="6"/>' +
          '<text class="lbl" x="' + (l.right ? l.x + 11 : l.x - 11) + '" y="' + l.ly + '" text-anchor="' +
          (l.right ? "start" : "end") + '">' + esc(p.model.model) + "</text></g>"
        );
      })
      .join("");

    return (
      '<div class="chart"><svg viewBox="0 0 ' + W + " " + H + '" role="group" aria-label="' +
      esc(p0label(xMode)) + '">' +
      grid +
      '<line class="axis" x1="' + M.l + '" x2="' + (W - M.r) + '" y1="' + (H - M.b) + '" y2="' + (H - M.b) + '"/>' +
      '<path class="frontier" d="' + frontPath + '"/>' +
      marks +
      '<text class="axis-label" x="' + (M.l + (W - M.l - M.r) / 2) + '" y="' + (H - 8) + '" text-anchor="middle">' +
      esc(xLabel) + "</text>" +
      '<text class="axis-label" transform="translate(14 ' + (M.t + (H - M.t - M.b) / 2) + ') rotate(-90)" text-anchor="middle">' +
      esc(models[0] ? models[0].stats.head.label : "Score") + "</text>" +
      '</svg><div class="tip" role="status" hidden></div></div>'
    );
  }

  function p0label(xMode) {
    return xMode === "latency" ? "Score against median latency, one point per model" : "Score against list price, one point per model";
  }

  function wireChart(root) {
    var chart = root.querySelector(".chart");
    if (!chart) return;
    var tip = chart.querySelector(".tip");
    function show(g) {
      var box = chart.getBoundingClientRect();
      var dot = g.querySelector(".dot").getBoundingClientRect();
      tip.textContent = g.getAttribute("data-tip");
      tip.hidden = false;
      var left = dot.left - box.left + dot.width / 2;
      tip.style.left = Math.max(8, Math.min(left, box.width - tip.offsetWidth - 8)) + "px";
      tip.style.top = Math.max(0, dot.top - box.top - tip.offsetHeight - 10) + "px";
    }
    function hide() {
      tip.hidden = true;
    }
    chart.querySelectorAll(".pt").forEach(function (g) {
      g.addEventListener("mouseenter", function () {
        show(g);
      });
      g.addEventListener("focus", function () {
        show(g);
      });
      g.addEventListener("mouseleave", hide);
      g.addEventListener("blur", hide);
    });
  }

  function viewLeaderboard(runInfo, params) {
    document.title = "Leaderboard · LiteOCR Benchmark";
    setNav("leaderboard", runInfo.run_id);
    return getRun(runInfo.run_id).then(function (run) {
      var sortKey = params.get("sort") || "score";
      var sortDir = params.get("dir") || (ASC_BY_DEFAULT.indexOf(sortKey) !== -1 ? "asc" : "desc");
      var xMode = params.get("x") === "latency" ? "latency" : "cost";
      var rulesRun = (run.models || []).some(function (m) {
        return isNum((m.summary || {}).rule_pass_rate);
      });
      var anyHead = (run.models || []).some(function (m) {
        return m.stats.head.headline;
      });
      var head0 = run.models[0] ? run.models[0].stats.head : { label: "Overall" };
      var headLabel = anyHead ? head0.label : "Overall";
      var headTitle = anyHead
        ? head0.title || "summary.headline from the result file"
        : "100 × mean per-document score (character similarity; table score on table-only documents; pass rate on rule documents); a failed call scores 0";

      var byScore = run.models.slice().sort(function (a, b) {
        return (b.stats.head.value || 0) - (a.stats.head.value || 0);
      });

      var reproduce =
        "liteocr bench run \\\n    --dataset benchmark/datasets/" +
        run.dataset.name +
        " \\\n    --models " +
        run.models
          .map(function (m) {
            return m.model;
          })
          .join(" ") +
        " \\\n    --concurrency 4 --save-outputs benchmark/results/outputs/<run_id>";

      var leader = byScore[0];
      var cheapestTop = null;
      byScore.forEach(function (m) {
        if (leader && m.stats.head.value >= leader.stats.head.value - 1) {
          if (!cheapestTop || (m.summary.cost_per_1k_pages_usd || Infinity) < (cheapestTop.summary.cost_per_1k_pages_usd || Infinity)) cheapestTop = m;
        }
      });

      view.innerHTML =
        '<div class="page-head"><div><p class="eyebrow">Open benchmark · every number is inspectable</p>' +
        "<h1>Leaderboard</h1></div>" +
        '<div class="toolbar">' +
        runSelector(runInfo) +
        "</div></div>" +
        runMeta(run) +
        (leader
          ? '<div class="stat-row">' +
            '<div class="stat"><span class="stat-k">Top ' + esc(headLabel.toLowerCase()) + "</span>" +
            '<span class="stat-v">' + esc(fixed(leader.stats.head.value, 1)) + "</span>" +
            '<span class="stat-s"><code>' + esc(leader.model) + "</code></span></div>" +
            (cheapestTop && cheapestTop !== leader
              ? '<div class="stat"><span class="stat-k">Within 1 point, cheapest</span><span class="stat-v">' +
                esc(moneyFmt(cheapestTop.summary.cost_per_1k_pages_usd)) +
                '</span><span class="stat-s"><code>' + esc(cheapestTop.model) + "</code> per 1k pages</span></div>"
              : "") +
            '<div class="stat"><span class="stat-k">Documents</span><span class="stat-v">' +
            esc(run.dataset.documents) +
            '</span><span class="stat-s">' + esc((run.sources || []).join(" + ")) + "</span></div>" +
            "</div>"
          : "") +
        sortableTable({
          id: "leaderboard",
          columns: leaderboardColumns(run, rulesRun, headLabel, headTitle),
          rows: run.models,
          sortKey: sortKey,
          sortDir: sortDir,
          caption:
            (anyHead
              ? "<strong>" + esc(headLabel) + "</strong> is <code>summary.headline</code> from the result file. "
              : "<strong>Overall</strong> = 100 × mean per-document score. ") +
            "Click a heading to sort, a model to inspect it document by document. " +
            "p90 is computed here from the per-document latencies; everything else is read from " +
            "<code>" + esc(runInfo.file) + "</code>.",
          rowClass: function (row) {
            return row === leader ? "leader" : "";
          },
        }) +
        '<section class="block"><div class="panel-title"><h2>Score vs ' +
        (xMode === "latency" ? "latency" : "cost") +
        "</h2>" +
        '<div class="seg" role="group" aria-label="X axis">' +
        '<button type="button" data-x="cost" aria-pressed="' + (xMode === "cost") + '">Cost</button>' +
        '<button type="button" data-x="latency" aria-pressed="' + (xMode === "latency") + '">Latency</button>' +
        "</div></div>" +
        scatterChart(run, byScore, xMode) +
        '<p class="hint">Up and to the left is better. The line joins the frontier: models no other model beats on both axes. ' +
        "Prices are public list prices, latency is client-measured p50.</p></section>" +
        breakdownTables(run, byScore) +
        '<div class="grid-2">' +
        '<section class="panel"><h2>Methodology</h2>' +
        "<p>Scores are deterministic text comparisons — no LLM judge. Predictions and ground truth are " +
        "normalised (NFKC, markdown syntax stripped, quotes and dashes straightened, whitespace collapsed" +
        (run.normalize && run.normalize.case_insensitive ? ", lowercased" : "") +
        "), then compared:</p><ul>" +
        "<li><strong>Per document</strong>: character similarity <code>1 − levenshtein / max(len)</code>; " +
        "on a <em>table-only</em> document the table score; on a <em>rules</em> document the share of assertions that pass. A failed call scores 0.</li>" +
        "<li><strong>CER / WER</strong> are edit rates over characters and whitespace tokens.</li>" +
        "<li><strong>Order</strong> is Kendall-τ-style agreement on the order of lines present in both texts.</li>" +
        "<li><strong>Table</strong> is character similarity restricted to markdown table rows.</li>" +
        "<li><strong>Latency</strong> is measured from the client with caches disabled; <strong>$/1k pages</strong> uses public list prices.</li></ul>" +
        '<p>Full definitions and caveats: <a href="' + METHODOLOGY + '" rel="noopener noreferrer">benchmark/README.md</a>.</p>' +
        "</section>" +
        '<section class="panel"><h2>Reproduce this run</h2>' +
        "<p>Install the CLI, set the provider keys you want to test, then:</p>" +
        codeBlock(reproduce) +
        '<p class="hint">Score one prediction offline: <code>liteocr bench score prediction.md truth.md</code>. ' +
        "Every document page has the exact commands for that document.</p>" +
        "</section></div>";

      wireSorting(view, function (key, dir) {
        return listHref("leaderboard", run.run_id, { sort: key, dir: dir, x: params.get("x") });
      });
      wireCopyButtons(view);
      wireRunSelect(view, "leaderboard");
      wireChart(view);
      view.querySelectorAll("button[data-x]").forEach(function (b) {
        b.addEventListener("click", function () {
          go(listHref("leaderboard", run.run_id, { sort: params.get("sort"), dir: params.get("dir"), x: b.getAttribute("data-x") }), true);
        });
      });
      KEYS = {};
      return "leaderboard|" + run.run_id;
    });
  }

  /* -------------------------------------------------------------- documents */

  /** The documents of `manifest` that `run` scored, filtered by src/cat/q. */
  function docList(run, manifest, params) {
    var src = params.get("src") || "";
    var cat = params.get("cat") || "";
    var q = (params.get("q") || "").toLowerCase();
    var scored = {};
    ((run.models[0] || {}).docs || []).forEach(function (d) {
      scored[d.id] = true;
    });
    return (manifest.documents || []).filter(function (doc) {
      if (!scored[doc.id]) return false;
      if (src && sourceOf(doc.id, run.dataset.name) !== src) return false;
      if (cat && doc.category !== cat) return false;
      if (q) {
        var hay = (doc.id + " " + doc.category + " " + (doc.tags || []).join(" ")).toLowerCase();
        if (hay.indexOf(q) === -1) return false;
      }
      return true;
    });
  }

  function recordsByDoc(run) {
    var byDoc = {};
    run.models.forEach(function (model) {
      (model.docs || []).forEach(function (doc) {
        (byDoc[doc.id] = byDoc[doc.id] || {})[model.model] = doc;
      });
    });
    return byDoc;
  }

  function viewDocuments(runInfo, params) {
    document.title = "Documents · LiteOCR Benchmark";
    setNav("documents", runInfo.run_id);
    return Promise.all([getRun(runInfo.run_id), getManifest(runInfo.dataset.name)]).then(function (loaded) {
      var run = loaded[0];
      var manifest = loaded[1];
      var src = params.get("src") || "";
      var cat = params.get("cat") || "";
      var models = run.models.slice().sort(function (a, b) {
        return (b.stats.head.value || 0) - (a.stats.head.value || 0);
      });
      var byDoc = recordsByDoc(run);
      var filters = { src: src, cat: cat, q: params.get("q") };

      var rows = docList(run, manifest, params).map(function (doc) {
        var scores = {};
        var sum = 0;
        var count = 0;
        models.forEach(function (model) {
          var rec = byDoc[doc.id] && byDoc[doc.id][model.model];
          var value = docScore(rec);
          scores[model.model] = { value: value, error: rec && rec.error, rec: rec };
          if (isNum(value)) {
            sum += value;
            count += 1;
          }
        });
        var any = (byDoc[doc.id] && byDoc[doc.id][models[0].model]) || {};
        return { doc: doc, scores: scores, mean: count ? sum / count : null, tableOnly: any.table_only };
      });

      var all = [];
      rows.forEach(function (row) {
        models.forEach(function (model) {
          all.push(row.scores[model.model].value);
        });
      });
      var hue = heatScale(all);
      var topModel = models[0] ? models[0].model : "";

      var columns = [
        {
          key: "id",
          label: "Document",
          asc: true,
          rowHeader: true,
          value: function (row) {
            return row.doc.id;
          },
          cell: function (row) {
            return {
              html:
                '<a href="' +
                esc(docHref(run.run_id, topModel, row.doc.id, filters)) +
                '">' +
                esc(row.doc.id) +
                "</a>" +
                (isRulesDoc(row.doc) ? ' <span class="tag">rules</span>' : "") +
                (row.tableOnly ? ' <span class="tag">table-only</span>' : ""),
              cls: "doc",
            };
          },
        },
        {
          key: "category",
          label: "Category",
          asc: true,
          value: function (row) {
            return row.doc.category;
          },
          cell: function (row) {
            return { html: esc(row.doc.category), cls: "muted" };
          },
        },
        {
          key: "mean",
          label: "Mean",
          num: true,
          title: "Mean score across every model in this run",
          value: function (row) {
            return row.mean;
          },
          cell: function (row) {
            return { html: esc(fixed(row.mean, 1)), cls: "primary" };
          },
        },
      ].concat(
        models.map(function (model) {
          return {
            key: "m:" + model.model,
            label: model.model,
            num: true,
            title: "Score of " + model.model + " on this document",
            value: function (row) {
              return row.scores[model.model].value;
            },
            cell: function (row) {
              var entry = row.scores[model.model];
              if (entry.error) return { html: "failed", cls: "fail", title: entry.error };
              if (!isNum(entry.value)) return { html: "—", cls: "muted" };
              return {
                html: '<a href="' + esc(docHref(run.run_id, model.model, row.doc.id, filters)) + '">' + esc(fixed(entry.value, 1)) + "</a>",
                cls: "heat",
                style: heatStyle(hue(entry.value)),
              };
            },
          };
        })
      );

      var sources = run.sources || [];
      var cats = src ? run.catsBySource[src] || [] : Object.keys(
        sources.reduce(function (acc, s) {
          (run.catsBySource[s] || []).forEach(function (c) {
            acc[c] = 1;
          });
          return acc;
        }, {})
      ).sort();

      function options(list, current, allLabel) {
        return (
          '<option value="">' + esc(allLabel) + "</option>" +
          list
            .map(function (name) {
              return '<option value="' + esc(name) + '"' + (name === current ? " selected" : "") + ">" + esc(name) + "</option>";
            })
            .join("")
        );
      }

      var sortKey = params.get("sort") || "id";
      var sortDir = params.get("dir") || (sortKey === "id" || sortKey === "category" ? "asc" : "desc");

      view.innerHTML =
        '<div class="page-head"><div><p class="eyebrow">' + esc(run.dataset.name) + " v" + esc(run.dataset.version) +
        "</p><h1>Documents</h1></div>" +
        '<div class="toolbar">' +
        runSelector(runInfo) +
        (sources.length > 1
          ? '<div class="field"><label for="src-select">Source</label><select id="src-select">' + options(sources, src, "All sources") + "</select></div>"
          : "") +
        '<div class="field"><label for="cat-select">Category</label><select id="cat-select">' + options(cats, cat, "All categories") + "</select></div>" +
        '<div class="field"><label for="doc-search">Filter</label><input id="doc-search" type="search" placeholder="id or tag" value="' +
        esc(params.get("q") || "") +
        '" /></div></div></div>' +
        '<p class="hint">' +
        rows.length +
        " of " +
        (manifest.documents || []).length +
        " documents. Open one to see the page, every model's output, the diff or rule checklist, and the layout boxes.</p>" +
        sortableTable({
          columns: columns,
          rows: rows,
          sortKey: sortKey,
          sortDir: sortDir,
          caption:
            "Per-document score (0–100) for each model: character similarity, table score on table-only documents, " +
            "assertion pass rate on rule documents. Click a score to open that document with that model.",
        });

      function refilter(values) {
        var next = { src: src, cat: cat, q: params.get("q"), sort: params.get("sort"), dir: params.get("dir") };
        Object.keys(values).forEach(function (k) {
          next[k] = values[k];
        });
        go(listHref("documents", run.run_id, next), true);
      }
      wireSorting(view, function (key, dir) {
        return listHref("documents", run.run_id, { src: src, cat: cat, q: params.get("q"), sort: key, dir: dir });
      });
      wireRunSelect(view, "documents");
      var srcSelect = view.querySelector("#src-select");
      if (srcSelect)
        srcSelect.addEventListener("change", function () {
          refilter({ src: srcSelect.value, cat: "" });
        });
      var catSelect = view.querySelector("#cat-select");
      catSelect.addEventListener("change", function () {
        refilter({ cat: catSelect.value });
      });
      var search = view.querySelector("#doc-search");
      var timer = null;
      search.addEventListener("input", function () {
        clearTimeout(timer);
        timer = setTimeout(function () {
          refilter({ q: search.value });
          var again = document.getElementById("doc-search");
          if (again) {
            again.focus();
            again.setSelectionRange(again.value.length, again.value.length);
          }
        }, 220);
      });
      KEYS = {};
      return "documents|" + run.run_id;
    });
  }

  /* ---------------------------------------------------------- word-level diff */

  function tokenize(text) {
    var tokens = [];
    var re = /\S+/g;
    var match;
    var last = 0;
    while ((match = re.exec(text)) !== null) {
      tokens.push({ word: match[0], pre: text.slice(last, match.index) });
      last = match.index + match[0].length;
    }
    return { tokens: tokens, tail: text.slice(last) };
  }

  /**
   * Comparison key: mirrors the benchmark's normalisation closely enough to be fair. Inline
   * HTML tags are dropped (the scorer strips them), and a token that is only markdown syntax
   * (`|`, `---`, `|-|-|`, `#`) gets the empty key: it is shown but never counted as a change.
   */
  function tokenKey(word) {
    var key = word.replace(/<\/?[A-Za-z][^>]*>/g, "").toLowerCase();
    if (/^[|:\-#*_>`~+]*$/.test(key)) return "";
    if (key.normalize) key = key.normalize("NFKC");
    key = key.replace(/[‘’]/g, "'").replace(/[“”]/g, '"').replace(/[–—]/g, "-");
    var stripped = key.replace(/^[#*_>`~|+-]+/, "").replace(/[#*_`~|]+$/, "");
    return stripped || key;
  }

  /** Ordered edit script between two key arrays. Returns [{t:'eq'|'del'|'ins', ai, bi}]. */
  function diffKeys(a, b) {
    var n = a.length;
    var m = b.length;
    var start = 0;
    while (start < n && start < m && a[start] === b[start]) start++;
    var end = 0;
    while (end < n - start && end < m - start && a[n - 1 - end] === b[m - 1 - end]) end++;
    var ops = [];
    var i;
    for (i = 0; i < start; i++) ops.push({ t: "eq", ai: i, bi: i });
    var an = n - end - start;
    var bn = m - end - start;
    if (an > 0 || bn > 0) {
      if ((an + 1) * (bn + 1) > DIFF_CELL_CAP) {
        for (i = 0; i < an; i++) ops.push({ t: "del", ai: start + i });
        for (i = 0; i < bn; i++) ops.push({ t: "ins", bi: start + i });
      } else {
        var width = bn + 1;
        var lcs = new Int32Array((an + 1) * width);
        var x, y;
        for (x = an - 1; x >= 0; x--) {
          for (y = bn - 1; y >= 0; y--) {
            if (a[start + x] === b[start + y]) lcs[x * width + y] = lcs[(x + 1) * width + y + 1] + 1;
            else {
              var down = lcs[(x + 1) * width + y];
              var right = lcs[x * width + y + 1];
              lcs[x * width + y] = down >= right ? down : right;
            }
          }
        }
        x = 0;
        y = 0;
        while (x < an && y < bn) {
          if (a[start + x] === b[start + y]) {
            ops.push({ t: "eq", ai: start + x, bi: start + y });
            x++;
            y++;
          } else if (lcs[(x + 1) * width + y] >= lcs[x * width + y + 1]) {
            ops.push({ t: "del", ai: start + x });
            x++;
          } else {
            ops.push({ t: "ins", bi: start + y });
            y++;
          }
        }
        for (; x < an; x++) ops.push({ t: "del", ai: start + x });
        for (; y < bn; y++) ops.push({ t: "ins", bi: start + y });
      }
    }
    for (i = end - 1; i >= 0; i--) ops.push({ t: "eq", ai: n - 1 - i, bi: m - 1 - i });
    return ops;
  }

  /** Keys of the significant tokens, plus a map back to the token index. */
  function significant(parsed) {
    var keys = [];
    var at = [];
    parsed.tokens.forEach(function (tok, i) {
      var key = tokenKey(tok.word);
      if (key) {
        keys.push(key);
        at.push(i);
      }
    });
    return { keys: keys, at: at };
  }

  function wordDiff(truth, pred) {
    var t = tokenize(truth);
    var p = tokenize(pred);
    var ts = significant(t);
    var ps = significant(p);
    var ops = diffKeys(ts.keys, ps.keys).map(function (op) {
      return { t: op.t, ai: op.ai === undefined ? undefined : ts.at[op.ai], bi: op.bi === undefined ? undefined : ps.at[op.bi] };
    });
    var res = { t: t, p: p, ops: ops, eq: 0, del: 0, ins: 0, tMarks: [], pMarks: [] };
    ops.forEach(function (op) {
      if (op.t === "eq") res.eq++;
      else if (op.t === "del") {
        res.del++;
        res.tMarks[op.ai] = true;
      } else {
        res.ins++;
        res.pMarks[op.bi] = true;
      }
    });
    return res;
  }

  function renderToken(token, tag) {
    return esc(token.pre) + (tag ? "<" + tag + ">" + esc(token.word) + "</" + tag + ">" : esc(token.word));
  }

  function renderSide(parsed, marks, tag) {
    var out = "";
    for (var i = 0; i < parsed.tokens.length; i++) out += renderToken(parsed.tokens[i], marks[i] ? tag : null);
    return out + esc(parsed.tail);
  }

  function renderUnified(d) {
    var out = "";
    var nextPred = 0; // prediction tokens are emitted in order, syntax-only ones included
    function flushTo(bi) {
      for (; nextPred <= bi; nextPred++) out += renderToken(d.p.tokens[nextPred], d.pMarks[nextPred] ? "ins" : null);
    }
    d.ops.forEach(function (op) {
      if (op.t === "del") out += renderToken(d.t.tokens[op.ai], "del");
      else flushTo(op.bi);
    });
    flushTo(d.p.tokens.length - 1);
    return out + esc(d.p.tail);
  }

  /* ------------------------------------------- rule checker (mirror of bench.rs) */
  // A line-for-line port of `normalize`, `markdown_to_text` and `score_rules` in
  // crates/liteocr-core, so every assertion can be shown passing or failing. The recorded
  // `rules_passed` from the Rust scorer stays authoritative; the viewer says when they differ.

  function stripHtmlTags(s) {
    var out = "";
    for (var i = 0; i < s.length; i++) {
      var c = s[i];
      if (c === "<" && /[A-Za-z\/]/.test(s[i + 1] || "")) {
        var close = s.indexOf(">", i + 1);
        if (close !== -1) {
          out += " ";
          i = close;
          continue;
        }
        // Unterminated tag: bench.rs has consumed the rest, keeping only the `<`.
        out += "<";
        break;
      }
      out += c;
    }
    return out;
  }

  function markdownToText(md) {
    var lines = stripHtmlTags(md).split("\n");
    var out = [];
    lines.forEach(function (line) {
      if (line.charAt(line.length - 1) === "\r") line = line.slice(0, -1);
      var trimmed = line.replace(/\s+$/, "").replace(/^\s+/, "");
      if (trimmed.charAt(0) === "|" && /^[|\-: ]*$/.test(trimmed)) return;
      var s = trimmed.replace(/^#+/, "").replace(/^\s+/, "");
      if (trimmed.charAt(0) === "#" && !s) return;
      var prefixes = ["- ", "* ", "+ ", "> "];
      for (var i = 0; i < prefixes.length; i++) {
        if (s.indexOf(prefixes[i]) === 0) {
          s = s.slice(2);
          break;
        }
      }
      if (s.charAt(0) === "|") {
        s = s
          .replace(/^\|+|\|+$/g, "")
          .split("|")
          .map(function (c) {
            return c.trim();
          })
          .join(" ");
      }
      s = s.split("**").join("").split("__").join("").split("`").join("");
      out.push(s.trim());
    });
    return out.join("\n").replace(/\s+$/, "");
  }

  // Scorer v2 (SCORER_VERSION = 2 in bench.rs): HTML entities decoded after markdown stripping,
  // spaces touching punctuation dropped for rule matching, markdown *and* HTML tables, fuzzy
  // bag_of_sentences (0.8 per sentence, default threshold 0.8) and olmOCR `max_diffs`.
  var BAG_SENTENCE_MIN_SIMILARITY = 0.8;
  var BAG_DEFAULT_THRESHOLD = 0.8;
  var MAX_SPAN = 64;

  var NAMED_ENTITIES = {
    amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", ndash: "–", mdash: "—",
    lsquo: "‘", rsquo: "’", ldquo: "“", rdquo: "”", hellip: "…", deg: "°",
    plusmn: "±", times: "×", copy: "©", reg: "®", euro: "€", pound: "£",
    cent: "¢", sect: "§", para: "¶", middot: "·", bull: "•",
  };

  /** `tables::decode_entities`. */
  function decodeEntities(s) {
    if (s.indexOf("&") === -1) return s;
    var out = "";
    var rest = s;
    var i;
    while ((i = rest.indexOf("&")) !== -1) {
      out += rest.slice(0, i);
      var tail = rest.slice(i);
      var semi = tail.slice(0, 12).indexOf(";");
      var ch = null;
      if (semi !== -1) {
        var name = tail.slice(1, semi);
        if (name.charAt(0) === "#") {
          var num = name.slice(1);
          var code = null;
          if (/^[xX]/.test(num)) {
            if (/^\+?[0-9a-fA-F]+$/.test(num.slice(1))) code = parseInt(num.slice(1).replace("+", ""), 16);
          } else if (/^\+?[0-9]+$/.test(num)) {
            code = parseInt(num.replace("+", ""), 10);
          }
          if (code !== null && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff)) ch = String.fromCodePoint(code);
        } else if (Object.prototype.hasOwnProperty.call(NAMED_ENTITIES, name)) {
          ch = NAMED_ENTITIES[name];
        }
      }
      if (ch !== null) {
        out += ch;
        rest = tail.slice(semi + 1);
      } else {
        out += "&";
        rest = tail.slice(1);
      }
    }
    return out + rest;
  }

  function normalizeText(s, opts) {
    s = s.normalize ? s.normalize("NFKC") : s;
    if (opts.strip_markdown) s = decodeEntities(markdownToText(s));
    s = s.replace(/[‘’]/g, "'").replace(/[“”]/g, '"').replace(/[–—]/g, "-");
    if (opts.case_insensitive) s = s.toLowerCase();
    var out = "";
    var lastSpace = true;
    for (var i = 0; i < s.length; i++) {
      var c = s[i];
      if (/\s/.test(c)) {
        if (!lastSpace) {
          out += " ";
          lastSpace = true;
        }
      } else if (opts.strip_punctuation && /[!-\/:-@\[-`{-~]/.test(c)) {
        continue;
      } else {
        out += c;
        lastSpace = false;
      }
    }
    return out.trim();
  }

  /** `is_punct` in bench.rs. */
  var PUNCT_RE = /[!-\/:-@\[-`{-~¡§«¶·»¿‐-‧‰-⁞、-〃〈-】〔-〟।॥]/;

  /** `squeeze_punct_spaces`: drop every space that touches punctuation. */
  function squeezePunctSpaces(s) {
    var chars = Array.from(s);
    var out = "";
    for (var i = 0; i < chars.length; i++) {
      if (chars[i] === " ") {
        var prev = i > 0 ? chars[i - 1] : "";
        var next = i + 1 < chars.length ? chars[i + 1] : "";
        if ((prev && PUNCT_RE.test(prev)) || (next && PUNCT_RE.test(next))) continue;
      }
      out += chars[i];
    }
    return out;
  }

  function ruleNormalize(s, opts) {
    return squeezePunctSpaces(normalizeText(s, opts));
  }

  function splitRow(line) {
    var cells = [""];
    var escaped = false;
    var body = line.trim().replace(/^\|+/, "");
    for (var i = 0; i < body.length; i++) {
      var c = body[i];
      if (escaped) {
        if (c !== "|") cells[cells.length - 1] += "\\";
        cells[cells.length - 1] += c;
        escaped = false;
      } else if (c === "\\") escaped = true;
      else if (c === "|") cells.push("");
      else cells[cells.length - 1] += c;
    }
    if (escaped) cells[cells.length - 1] += "\\";
    if (cells.length > 1 && !cells[cells.length - 1].trim()) cells.pop();
    return cells.map(function (c) {
      return c.trim();
    });
  }

  /** `tables::markdown_tables`: raw cells, separator rows dropped. */
  function markdownTables(md) {
    var tables = [];
    var current = [];
    md.split("\n").forEach(function (line) {
      if (line.charAt(line.length - 1) === "\r") line = line.slice(0, -1);
      var t = line.trim();
      if (t.charAt(0) !== "|") {
        if (current.length) tables.push(current);
        current = [];
        return;
      }
      if (/^[|\-: ]*$/.test(t)) return;
      current.push(splitRow(t));
    });
    if (current.length) tables.push(current);
    return tables;
  }

  function asciiLower(s) {
    return s.replace(/[A-Z]+/g, function (m) {
      return m.toLowerCase();
    });
  }

  function findTableOpen(lower, from) {
    var at = from;
    for (;;) {
      var i = lower.indexOf("<table", at);
      if (i === -1) return -1;
      var next = lower.charAt(i + 6);
      if (next === "" || /[\t\n\f\r >\/]/.test(next)) return i;
      at = i + 6;
    }
  }

  function spanAttr(attrs, key) {
    var lower = asciiLower(attrs);
    var from = 0;
    var i;
    while ((i = lower.indexOf(key, from)) !== -1) {
      from = i + key.length;
      var beforeOk = i === 0 || !/[A-Za-z0-9]/.test(lower.charAt(i - 1));
      var rest = lower.slice(i + key.length).replace(/^\s+/, "");
      if (!beforeOk || rest.charAt(0) !== "=") continue;
      rest = rest.slice(1).replace(/^\s+/, "").replace(/^["']+/, "");
      var digits = /^[0-9]*/.exec(rest)[0];
      var n = digits ? parseInt(digits, 10) : NaN;
      return n >= 1 && n <= MAX_SPAN ? n : 1;
    }
    return 1;
  }

  function collapseWs(s) {
    return s.split(/\s+/).filter(Boolean).join(" ");
  }

  /** `tables::expand`: repeat spanning cells into every slot, pad, drop all-empty rows. */
  function expandGrid(rows) {
    var grid = [];
    rows.forEach(function (row, r) {
      while (grid.length <= r) grid.push([]);
      var c = 0;
      row.forEach(function (cell) {
        while (grid[r][c] !== undefined && grid[r][c] !== null) c++;
        var text = collapseWs(cell.text);
        for (var dr = 0; dr < cell.rowspan; dr++) {
          var rr = r + dr;
          while (grid.length <= rr) grid.push([]);
          for (var dc = 0; dc < cell.colspan; dc++) {
            var cc = c + dc;
            while (grid[rr].length <= cc) grid[rr].push(null);
            grid[rr][cc] = text;
          }
        }
        c += cell.colspan;
      });
    });
    var width = grid.reduce(function (w, row) {
      return Math.max(w, row.length);
    }, 0);
    return grid
      .map(function (row) {
        var out = [];
        for (var i = 0; i < width; i++) out.push(row[i] == null ? "" : row[i]);
        return out;
      })
      .filter(function (row) {
        return row.some(function (c) {
          return c !== "";
        });
      });
  }

  /** `tables::parse_html_table`: {grid, consumed}. */
  function parseHtmlTable(html) {
    var rows = [];
    var row = null;
    var cell = null;
    var depth = 0;
    var started = false;
    var i = 0;
    var textStart = 0;
    function closeCell() {
      if (cell) {
        (row = row || []).push(cell);
        cell = null;
      }
    }
    function closeRow() {
      closeCell();
      if (row && row.length) rows.push(row);
      row = null;
    }
    function addText(t) {
      if (cell) cell.text += t;
    }
    while (i < html.length) {
      if (html.charAt(i) !== "<") {
        i++;
        continue;
      }
      if (html.slice(i, i + 4) === "<!--") {
        addText(decodeEntities(html.slice(textStart, i)));
        var endc = html.indexOf("-->", i + 4);
        i = endc === -1 ? html.length : endc + 3;
        textStart = i;
        continue;
      }
      var rel = html.indexOf(">", i);
      if (rel === -1) break;
      var tagEnd = rel + 1;
      var inner = html.slice(i + 1, tagEnd - 1);
      var closing = inner.charAt(0) === "/";
      var rest = closing ? inner.slice(1) : inner;
      var nameLen = /^[A-Za-z0-9]*/.exec(rest)[0].length;
      var name = asciiLower(rest.slice(0, nameLen));
      var attrs = rest.slice(nameLen);
      if (!name || !/[A-Za-z]/.test(rest.charAt(0))) {
        i++;
        continue;
      }
      addText(decodeEntities(html.slice(textStart, i)));
      i = tagEnd;
      textStart = i;
      if (!closing && name === "table") {
        if (started) {
          depth++;
          addText(" ");
        } else started = true;
      } else if (closing && name === "table") {
        if (depth > 0) {
          depth--;
          addText(" ");
        } else {
          closeRow();
          return { grid: expandGrid(rows), consumed: tagEnd };
        }
      } else if (depth > 0) {
        addText(" ");
      } else if (name === "tr") {
        closeRow();
      } else if (!closing && (name === "td" || name === "th")) {
        closeCell();
        cell = { text: "", colspan: spanAttr(attrs, "colspan"), rowspan: spanAttr(attrs, "rowspan") };
      } else if (closing && (name === "td" || name === "th")) {
        closeCell();
      } else if (name === "br" || name === "p" || name === "li" || name === "div") {
        addText(" ");
      }
    }
    addText(decodeEntities(html.slice(Math.min(textStart, html.length))));
    closeRow();
    return { grid: expandGrid(rows), consumed: html.length };
  }

  /** `tables::extract_tables`: markdown and HTML tables in document order, raw cells. */
  function extractTables(doc) {
    var lower = asciiLower(doc);
    var out = [];
    var pos = 0;
    var start;
    while ((start = findTableOpen(lower, pos)) !== -1) {
      out = out.concat(markdownTables(doc.slice(pos, start)));
      var parsed = parseHtmlTable(doc.slice(start));
      if (parsed.grid.length) out.push(parsed.grid);
      pos = start + Math.max(parsed.consumed, 1);
    }
    return out.concat(markdownTables(doc.slice(Math.min(pos, doc.length))));
  }

  function parseTables(md, opts) {
    return extractTables(md).map(function (grid) {
      var rows = grid.map(function (row) {
        return row.map(function (c) {
          return ruleNormalize(c, opts);
        });
      });
      return { header: rows[0], rows: rows.slice(1) };
    });
  }

  function asNumber(s) {
    var cleaned = s.replace(/[,$%  ]/g, "");
    if (!cleaned) return null;
    var m = /^\((.*)\)$/.exec(cleaned);
    if (m) cleaned = "-" + m[1];
    if (!/^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(cleaned)) return null;
    var n = parseFloat(cleaned);
    return isFinite(n) ? n : null;
  }

  function valuesEqual(a, b) {
    if (a === b) return true;
    var x = asNumber(a);
    var y = asNumber(b);
    return x !== null && y !== null && Math.abs(x - y) <= 1e-6 * Math.max(Math.abs(x), Math.abs(y), 1);
  }

  function levenshtein(a, b) {
    if (!a.length) return b.length;
    if (!b.length) return a.length;
    var prev = [];
    var cur = [];
    for (var j = 0; j <= b.length; j++) prev.push(j);
    for (var i = 0; i < a.length; i++) {
      cur = [i + 1];
      for (j = 0; j < b.length; j++) cur.push(Math.min(prev[j + 1] + 1, cur[j] + 1, prev[j] + (a[i] === b[j] ? 0 : 1)));
      prev = cur;
    }
    return prev[b.length];
  }

  /** `approx_contains`: some substring of `hay` within `max` edits of `needle` (char arrays). */
  function approxContains(needle, hay, max) {
    var m = needle.length;
    if (m <= max) return true;
    var col = [];
    for (var i = 0; i <= m; i++) col.push(i);
    for (var j = 0; j < hay.length; j++) {
      var h = hay[j];
      var diag = 0;
      for (i = 1; i <= m; i++) {
        var up = col[i];
        var v = needle[i - 1] === h ? diag : 1 + Math.min(diag, up, col[i - 1]);
        diag = up;
        col[i] = v;
      }
      if (col[m] <= max) return true;
    }
    return false;
  }

  /** `approx_match_ends`: exclusive end positions of matches within `max` edits. */
  function approxMatchEnds(needle, hay, max) {
    var m = needle.length;
    var ends = [];
    var col = [];
    for (var i = 0; i <= m; i++) col.push(i);
    if (col[m] <= max) ends.push(0);
    for (var j = 0; j < hay.length; j++) {
      var h = hay[j];
      var diag = 0;
      for (i = 1; i <= m; i++) {
        var up = col[i];
        var v = needle[i - 1] === h ? diag : 1 + Math.min(diag, up, col[i - 1]);
        diag = up;
        col[i] = v;
      }
      if (col[m] <= max) ends.push(j + 1);
    }
    return ends;
  }

  /** `approx_match_start_range`: [earliest start, latest start] or null. */
  function approxMatchStartRange(needle, hay, max) {
    var ends = approxMatchEnds(Array.from(needle).reverse(), hay.slice().reverse(), max);
    if (!ends.length) return null;
    var n = hay.length;
    return [n - Math.max.apply(null, ends), n - Math.min.apply(null, ends)];
  }

  function containsWithin(hay, needle, k) {
    if (!k) return hay.indexOf(needle) !== -1;
    return approxContains(Array.from(needle), Array.from(hay), k);
  }

  function valuesClose(a, b, k) {
    return valuesEqual(a, b) || (k > 0 && levenshtein(Array.from(a), Array.from(b)) <= k);
  }

  function snippet(s) {
    var chars = Array.from(s);
    return chars.length <= 60 ? JSON.stringify(s) : JSON.stringify(chars.slice(0, 60).join("")) + "…";
  }

  function RuleChecker(prediction, opts) {
    var self = this;
    var texts = {};
    var chars = {};
    var tables = {};
    function optsFor(cs) {
      return { case_insensitive: !cs, strip_markdown: opts.strip_markdown, strip_punctuation: opts.strip_punctuation };
    }
    self.text = function (cs) {
      var k = cs ? 1 : 0;
      if (texts[k] === undefined) texts[k] = ruleNormalize(prediction, optsFor(cs));
      return texts[k];
    };
    self.chars = function (cs) {
      var k = cs ? 1 : 0;
      if (chars[k] === undefined) chars[k] = Array.from(self.text(cs));
      return chars[k];
    };
    self.tables = function (cs) {
      var k = cs ? 1 : 0;
      if (tables[k] === undefined) tables[k] = parseTables(prediction, optsFor(cs));
      return tables[k];
    };
    self.needle = function (s, cs) {
      return ruleNormalize(String(s), optsFor(cs));
    };
  }

  function sentencePresent(ck, sentence, cs) {
    if (!sentence) return false;
    if (ck.text(cs).indexOf(sentence) !== -1) return true;
    var needle = Array.from(sentence);
    var max = Math.floor((1 - BAG_SENTENCE_MIN_SIMILARITY) * needle.length);
    return max > 0 && approxContains(needle, ck.chars(cs), max);
  }

  /** {ok, detail, found?: bool[]} for one rule. */
  function checkRule(ck, rule) {
    var cs = !!rule.case_sensitive;
    var k = isNum(rule.max_diffs) ? rule.max_diffs : 0;
    var type = rule.type;
    if (type === "present" || type === "absent") {
      if (rule.text == null) return { ok: false, detail: "missing `text`" };
      var needle = ck.needle(rule.text, cs);
      if (!needle) return { ok: false, detail: "`text` " + snippet(rule.text) + " is empty after normalisation" };
      var found = k === 0 ? ck.text(cs).indexOf(needle) !== -1 : approxContains(Array.from(needle), ck.chars(cs), k);
      if (type === "present" && !found) return { ok: false, detail: "not found: " + snippet(needle) };
      if (type === "absent" && found) return { ok: false, detail: "present but must be absent: " + snippet(needle) };
      return { ok: true };
    }
    if (type === "order") {
      if (rule.before == null) return { ok: false, detail: "missing `before`" };
      if (rule.after == null) return { ok: false, detail: "missing `after`" };
      var before = ck.needle(rule.before, cs);
      var after = ck.needle(rule.after, cs);
      if (!before || !after) return { ok: false, detail: "`before` or `after` is empty after normalisation" };
      if (k > 0) {
        var bRange = approxMatchStartRange(before, ck.chars(cs), k);
        if (!bRange) return { ok: false, detail: "`before` not found within " + k + " edits: " + snippet(before) };
        var aRange = approxMatchStartRange(after, ck.chars(cs), k);
        if (!aRange) return { ok: false, detail: "`after` not found within " + k + " edits: " + snippet(after) };
        if (bRange[0] < aRange[1]) return { ok: true };
        return { ok: false, detail: snippet(after) + " occurs only before " + snippet(before) };
      }
      var hay = ck.text(cs);
      var b = hay.indexOf(before);
      if (b === -1) return { ok: false, detail: "`before` not found: " + snippet(before) };
      if (hay.slice(b + before.length).indexOf(after) !== -1) return { ok: true };
      if (hay.indexOf(after) !== -1) return { ok: false, detail: snippet(after) + " occurs only before " + snippet(before) };
      return { ok: false, detail: "`after` not found: " + snippet(after) };
    }
    if (type === "bag_of_sentences") {
      var sentences = rule.sentences || [];
      if (!sentences.length) return { ok: false, detail: "missing `sentences`" };
      var threshold = isNum(rule.threshold) ? rule.threshold : BAG_DEFAULT_THRESHOLD;
      var hits = 0;
      var marks = sentences.map(function (s) {
        var hit = sentencePresent(ck, ck.needle(s, cs), cs);
        if (hit) hits++;
        return hit;
      });
      var fraction = hits / sentences.length;
      if (fraction + 1e-9 >= threshold) return { ok: true, found: marks };
      return {
        ok: false,
        found: marks,
        detail: hits + "/" + sentences.length + " sentences present (" + fraction.toFixed(3) + " < threshold " + threshold.toFixed(3) + ")",
      };
    }
    if (type === "table_cell") {
      if (!rule.cell) return { ok: false, detail: "missing `cell`" };
      return checkTableCell(ck, rule.cell, cs, k);
    }
    return { ok: false, detail: "unknown rule type " + JSON.stringify(type) };
  }

  function checkTableCell(ck, cell, cs, k) {
    var value = ck.needle(cell.value == null ? "" : cell.value, cs);
    if (!value) return { ok: false, detail: "`cell.value` is empty after normalisation" };
    var rowH = cell.row_header != null ? ck.needle(cell.row_header, cs) : null;
    if (rowH === "") return { ok: false, detail: "`cell.row_header` is empty after normalisation" };
    var colH = cell.col_header != null ? ck.needle(cell.col_header, cs) : null;
    if (colH === "") return { ok: false, detail: "`cell.col_header` is empty after normalisation" };
    var tables = ck.tables(cs);
    if (!tables.length) return { ok: false, detail: "prediction has no table (markdown or HTML)" };
    var sawRow = false;
    var sawCol = false;
    var seen = [];
    for (var t = 0; t < tables.length; t++) {
      var table = tables[t];
      var col = null;
      if (colH !== null) {
        col = -1;
        for (var h = 0; h < table.header.length; h++) {
          if (containsWithin(table.header[h], colH, k)) {
            col = h;
            break;
          }
        }
        if (col === -1) continue;
        sawCol = true;
      }
      for (var r = 0; r < table.rows.length; r++) {
        var row = table.rows[r];
        if (rowH !== null && !row.some(function (c) { return containsWithin(c, rowH, k); })) continue;
        sawRow = true;
        if (col !== null) {
          if (col >= row.length) continue;
          if (valuesClose(row[col], value, k)) return { ok: true };
          seen.push(row[col]);
        } else if (row.some(function (c) { return valuesClose(c, value, k); }) || containsWithin(row.join(" "), value, k)) {
          return { ok: true };
        }
      }
    }
    if (colH !== null && !sawCol) return { ok: false, detail: "no column matching " + snippet(colH) };
    if (!sawRow) return { ok: false, detail: rowH !== null ? "no row matching " + snippet(rowH) : "prediction has no table rows" };
    if (seen.length) return { ok: false, detail: "cell is " + snippet(seen[0]) + " , expected " + snippet(value) };
    return { ok: false, detail: "no cell in the matched row holds " + snippet(value) };
  }

  function scoreRules(prediction, rules, opts) {
    var ck = new RuleChecker(prediction, opts);
    var passed = 0;
    var results = rules.map(function (rule) {
      var r = checkRule(ck, rule);
      if (r.ok) passed++;
      return r;
    });
    return { passed: passed, total: rules.length, results: results };
  }

  /* ------------------------------------------------------- source viewer */

  var pdfLib = null;

  function loadPdfJs() {
    if (pdfLib) return pdfLib;
    pdfLib = new Promise(function (resolve, reject) {
      var script = document.createElement("script");
      script.src = PDFJS.lib;
      script.integrity = PDFJS.libSri;
      script.crossOrigin = "anonymous";
      script.referrerPolicy = "no-referrer";
      script.onload = function () {
        resolve(window.pdfjsLib);
      };
      script.onerror = function () {
        reject(new Error("pdf.js could not be loaded from cdnjs"));
      };
      document.head.appendChild(script);
    }).then(function (lib) {
      if (!lib) throw new Error("pdf.js did not initialise");
      // Fetch the worker with the same integrity check, then run it from a blob URL.
      return fetch(PDFJS.worker, { integrity: PDFJS.workerSri, mode: "cors", credentials: "omit" })
        .then(function (resp) {
          if (!resp.ok) throw new Error("pdf.js worker HTTP " + resp.status);
          return resp.blob();
        })
        .then(function (blob) {
          lib.GlobalWorkerOptions.workerSrc = URL.createObjectURL(blob);
          return lib;
        });
    });
    pdfLib.catch(function () {
      pdfLib = null;
    });
    return pdfLib;
  }

  function groupOf(type) {
    for (var i = 0; i < BLOCK_GROUPS.length; i++) if (BLOCK_GROUPS[i].types.indexOf(type) !== -1) return BLOCK_GROUPS[i];
    return BLOCK_GROUPS[BLOCK_GROUPS.length - 1];
  }

  /** Blocks with a usable bbox from a unified ParseResponse, grouped by 1-based page. */
  function blocksByPage(response) {
    var pages = {};
    var count = 0;
    function take(block, fallbackPage) {
      var b = block && block.bbox;
      if (!b || ![b.x0, b.y0, b.x1, b.y1].every(isNum)) return;
      var page = block.page_number || fallbackPage || 1;
      (pages[page] = pages[page] || []).push(block);
      count++;
    }
    ((response && response.pages) || []).forEach(function (page) {
      (page.blocks || []).forEach(function (block) {
        take(block, page.page_number);
      });
    });
    ((response && response.blocks) || []).forEach(function (block) {
      take(block, 1);
    });
    return { pages: pages, count: count };
  }

  /**
   * The page panel: renders the input (an image, or a PDF page through pdf.js) and draws a
   * model's layout blocks over it. Built once per document and kept while the model or tab
   * changes, so switching models never re-renders the page.
   */
  function SourceViewer(key, opts) {
    var self = this;
    self.key = key;
    self.page = 1;
    self.pages = opts.pages || 1;
    self.overlay = null;
    self.overlayOn = true;
    self.el = document.createElement("div");
    self.el.className = "sv";
    self.el.innerHTML =
      '<div class="sv-bar">' +
      '<div class="pager" hidden><button type="button" class="btn sm" data-page="-1" aria-label="Previous page">‹</button>' +
      '<span class="pager-label"></span>' +
      '<button type="button" class="btn sm" data-page="1" aria-label="Next page">›</button></div>' +
      '<button type="button" class="btn sm ov-toggle" aria-pressed="false" disabled>Layout boxes</button>' +
      '<a class="sv-open" href="' + esc(opts.inputUrl) + '" rel="noopener" target="_blank">Open original ↗</a>' +
      "</div>" +
      '<div class="sv-stage"><div class="sv-page"><div class="sv-media"><p class="loading">Loading page…</p></div>' +
      '<svg class="ov" viewBox="0 0 1000 1000" preserveAspectRatio="none" aria-hidden="true"></svg></div></div>' +
      '<div class="sv-legend" hidden></div>' +
      '<p class="sv-note hint"></p>';
    self.media = self.el.querySelector(".sv-media");
    self.svg = self.el.querySelector("svg.ov");
    self.note = self.el.querySelector(".sv-note");
    self.legend = self.el.querySelector(".sv-legend");
    self.toggle = self.el.querySelector(".ov-toggle");
    self.pager = self.el.querySelector(".pager");
    self.opts = opts;

    self.pager.querySelectorAll("button").forEach(function (b) {
      b.addEventListener("click", function () {
        self.onPage(self.page + Number(b.getAttribute("data-page")));
      });
    });
    self.toggle.addEventListener("click", function () {
      self.onOverlay(!self.overlayOn);
    });
    // Hooks the inspector replaces so page / overlay changes land in the URL.
    self.onPage = function (n) {
      self.showPage(n);
    };
    self.onOverlay = function (on) {
      self.setOverlayOn(on);
    };

    if (opts.isPdf) self.initPdf();
    else self.initImage(opts.imageUrl);
  }

  SourceViewer.prototype.initImage = function (src) {
    var self = this;
    if (!src) {
      self.media.innerHTML = '<p class="empty">No inline preview. <a href="' + esc(self.opts.inputUrl) + '">Open the file</a>.</p>';
      return;
    }
    var img = new Image();
    img.alt = "Input document " + self.opts.docId;
    img.decoding = "async";
    img.onload = function () {
      if (self.pdfShown) return; // pdf.js already painted the real page
      self.media.innerHTML = "";
      self.media.appendChild(img);
      self.ready = true;
      self.drawOverlay();
    };
    img.onerror = function () {
      self.media.innerHTML = '<p class="empty">The input could not be displayed. <a href="' + esc(self.opts.inputUrl) + '">Open the file</a>.</p>';
    };
    img.src = src;
    self.updatePager();
  };

  SourceViewer.prototype.initPdf = function () {
    var self = this;
    self.renderToken = 0;
    self.updatePager();
    // Paint the build-time page-1 PNG at once; pdf.js replaces it when it has rendered.
    if (self.opts.previewUrl) self.initImage(self.opts.previewUrl);
    loadPdfJs()
      .then(function (lib) {
        return lib.getDocument({ url: self.opts.inputUrl, isEvalSupported: false }).promise;
      })
      .then(function (pdf) {
        self.pdf = pdf;
        self.pages = pdf.numPages;
        self.updatePager();
        return self.renderPdfPage();
      })
      .catch(function (err) {
        // Fall back to the page-1 PNG the build rendered, if any.
        self.pdf = null;
        self.pdfFailed = true;
        if (self.opts.previewUrl) {
          self.updatePager();
          self.pdfNote = "pdf.js is unavailable (" + (err && err.message ? err.message : err) + "), so this is the build-time page-1 preview.";
          self.updateNote();
          self.drawOverlay();
        } else {
          self.media.innerHTML = '<p class="empty">This PDF cannot be rendered here. <a href="' + esc(self.opts.inputUrl) + '">Download it</a>.</p>';
        }
      });
  };

  SourceViewer.prototype.renderPdfPage = function () {
    var self = this;
    if (!self.pdf) return Promise.resolve();
    var token = ++self.renderToken;
    var number = Math.max(1, Math.min(self.page, self.pages));
    return self.pdf.getPage(number).then(function (page) {
      if (token !== self.renderToken) return;
      var base = page.getViewport({ scale: 1 });
      var cssWidth = Math.max(320, Math.min(self.el.clientWidth || 640, 1100));
      var ratio = Math.min(window.devicePixelRatio || 1, 2);
      var viewport = page.getViewport({ scale: (cssWidth / base.width) * ratio });
      var canvas = document.createElement("canvas");
      canvas.width = Math.floor(viewport.width);
      canvas.height = Math.floor(viewport.height);
      canvas.setAttribute("role", "img");
      canvas.setAttribute("aria-label", "Page " + number + " of input document " + self.opts.docId);
      return page.render({ canvasContext: canvas.getContext("2d"), viewport: viewport }).promise.then(function () {
        if (token !== self.renderToken) return;
        self.pdfShown = true;
        self.media.innerHTML = "";
        self.media.appendChild(canvas);
        self.ready = true;
        self.drawOverlay();
      });
    });
  };

  SourceViewer.prototype.updatePager = function () {
    var multi = this.pages > 1 && !this.pdfFailed;
    this.pager.hidden = !multi;
    this.pager.querySelector(".pager-label").textContent = "Page " + this.page + " / " + this.pages;
    this.pager.querySelector('[data-page="-1"]').disabled = this.page <= 1;
    this.pager.querySelector('[data-page="1"]').disabled = this.page >= this.pages;
  };

  SourceViewer.prototype.showPage = function (n) {
    n = Math.max(1, Math.min(n || 1, this.pages));
    if (n === this.page) return;
    this.page = n;
    this.updatePager();
    if (this.pdf) this.renderPdfPage();
    else this.drawOverlay();
  };

  SourceViewer.prototype.setOverlay = function (overlay, message) {
    this.overlay = overlay;
    this.toggle.disabled = !overlay || !overlay.count;
    this.ovMessage = message || "";
    this.updateNote();
    this.drawOverlay();
  };

  SourceViewer.prototype.updateNote = function () {
    this.note.innerHTML = (this.ovMessage || "") + (this.pdfNote ? " " + esc(this.pdfNote) : "");
  };

  SourceViewer.prototype.setOverlayOn = function (on) {
    this.overlayOn = !!on;
    this.drawOverlay();
  };

  SourceViewer.prototype.drawOverlay = function () {
    var active = !!(this.overlay && this.overlay.count && this.overlayOn);
    this.toggle.setAttribute("aria-pressed", String(active));
    var blocks = active ? this.overlay.pages[this.page] || [] : [];
    // The saved image preview of a PDF only shows page 1.
    if (this.opts.isPdf && !this.pdfShown && this.page !== 1) blocks = [];
    var used = {};
    this.svg.innerHTML = blocks
      .map(function (block, i) {
        var b = block.bbox;
        var g = groupOf(block.type);
        used[g.key] = g;
        var x = Math.min(b.x0, b.x1) * 1000;
        var y = Math.min(b.y0, b.y1) * 1000;
        var w = Math.abs(b.x1 - b.x0) * 1000;
        var h = Math.abs(b.y1 - b.y0) * 1000;
        var text = String(block.text || block.content || "").replace(/\s+/g, " ").slice(0, 160);
        return (
          '<rect class="bt bt-' + g.key + '" x="' + x.toFixed(1) + '" y="' + y.toFixed(1) + '" width="' + w.toFixed(1) +
          '" height="' + h.toFixed(1) + '" vector-effect="non-scaling-stroke"><title>' +
          esc("#" + (i + 1) + " " + (block.type || "?") + (isNum(block.confidence) ? " (" + block.confidence.toFixed(2) + ")" : "") + " — " + text) +
          "</title></rect>"
        );
      })
      .join("");
    this.svg.style.display = active ? "" : "none";
    var keys = BLOCK_GROUPS.filter(function (g) {
      return used[g.key];
    });
    this.legend.hidden = !active || !keys.length;
    this.legend.innerHTML = keys
      .map(function (g) {
        return '<span class="lg"><span class="sw bt-' + g.key + '"></span>' + esc(g.label) + "</span>";
      })
      .join("") + (active ? '<span class="lg muted">' + blocks.length + " blocks on this page</span>" : "");
  };

  /* ---------------------------------------------------------- inspector */

  function metricTile(label, value, title) {
    return (
      '<div class="metric"' + (title ? ' title="' + esc(title) + '"' : "") + "><dt>" + esc(label) + "</dt><dd>" + esc(value) + "</dd></div>"
    );
  }

  function metricTiles(rec) {
    var m = (rec && rec.metrics) || {};
    var rules = isNum(m.rule_pass_rate);
    var score = docScore(rec);
    var tiles = metricTile("Score", fixed(score, 2), "Primary per-document score: " + scoreBasis(rec));
    if (rules) {
      tiles +=
        metricTile("Rules passed", (m.rules_passed != null ? m.rules_passed : "—") + " / " + (m.rules_total != null ? m.rules_total : "—")) +
        metricTile("Order", fixed(m.order_score, 3), "Pass rate of the order rules");
    } else {
      tiles +=
        metricTile("Char sim", fixed(m.char_similarity, 4)) +
        metricTile("Table", fixed(m.table_score, 4)) +
        metricTile("TEDS", fixed(m.teds_grid, 4), "TEDS on the row/cell grid: table structure plus cell content (scorer v2)") +
        metricTile("CER", fixed(m.cer, 4)) +
        metricTile("WER", fixed(m.wer, 4)) +
        metricTile("Word F1", fixed(m.word_f1, 4)) +
        metricTile("Order", fixed(m.order_score, 4)) +
        metricTile("Chars", (m.pred_chars != null ? m.pred_chars : "—") + " / " + (m.truth_chars != null ? m.truth_chars : "—"), "Predicted / truth characters after normalisation");
    }
    tiles +=
      metricTile("Latency", rec ? msFmt(rec.latency_ms) : "—", "Client-measured, caches disabled") +
      metricTile("Cost", rec && isNum(rec.cost_usd) ? "$" + rec.cost_usd.toFixed(4) : "—", "List price for this document");
    return '<dl class="metrics-grid">' + tiles + "</dl>";
  }

  function ruleExpected(rule, result) {
    if (rule.type === "present") return '<span class="rk">must contain</span> <q>' + esc(rule.text) + "</q>";
    if (rule.type === "absent") return '<span class="rk">must not contain</span> <q>' + esc(rule.text) + "</q>";
    if (rule.type === "order")
      return '<q>' + esc(rule.before) + '</q> <span class="rk">then</span> <q>' + esc(rule.after) + "</q>";
    if (rule.type === "table_cell") {
      var c = rule.cell || {};
      return (
        '<span class="rk">cell</span> ' +
        (c.row_header != null ? "row <q>" + esc(c.row_header) + "</q> " : "") +
        (c.col_header != null ? "column <q>" + esc(c.col_header) + "</q> " : "") +
        '<span class="rk">=</span> <q>' + esc(c.value) + "</q>"
      );
    }
    if (rule.type === "bag_of_sentences") {
      var sentences = rule.sentences || [];
      var found = (result && result.found) || [];
      var order = sentences.map(function (s, i) {
        return i;
      });
      // Missing sentences first: they are what explains a failure.
      order.sort(function (a, b) {
        return (found[a] ? 1 : 0) - (found[b] ? 1 : 0) || a - b;
      });
      var shown = order.slice(0, BAG_SENTENCE_CAP);
      var rest = sentences.length - shown.length;
      return (
        '<span class="rk">at least ' + esc(fixed((isNum(rule.threshold) ? rule.threshold : BAG_DEFAULT_THRESHOLD) * 100, 0)) + "% of " + sentences.length + " sentences</span>" +
        '<ul class="bag">' +
        shown
          .map(function (i) {
            return '<li class="' + (found[i] ? "ok" : "no") + '"><span class="mk">' + (found[i] ? "✓" : "✗") + "</span>" + esc(sentences[i]) + "</li>";
          })
          .join("") +
        (rest > 0 ? '<li class="more">… ' + rest + " more</li>" : "") +
        "</ul>"
      );
    }
    return "<code>" + esc(JSON.stringify(rule)) + "</code>";
  }

  function rulesChecklist(rules, scored, rec, filter, limit) {
    if (!rules) return '<p class="empty">The assertion file for this document could not be loaded.</p>';
    if (!scored) return '<p class="empty">No saved output to check the assertions against.</p>';
    var m = (rec && rec.metrics) || {};
    var matches = m.rules_passed === scored.passed && m.rules_total === scored.total;
    var failing = scored.total - scored.passed;
    var items = [];
    rules.forEach(function (rule, i) {
      var r = scored.results[i];
      if (filter === "fail" && r.ok) return;
      if (filter === "pass" && !r.ok) return;
      items.push({ rule: rule, r: r, i: i });
    });
    var byType = {};
    rules.forEach(function (rule, i) {
      var t = (byType[rule.type] = byType[rule.type] || { p: 0, n: 0 });
      t.n++;
      if (scored.results[i].ok) t.p++;
    });
    return (
      '<div class="rules-summary">' +
      '<span class="big">' + scored.passed + " / " + scored.total + "</span> assertions pass" +
      (m.rules_total != null
        ? matches
          ? ' <span class="badge ok">✓ matches the recorded score</span>'
          : ' <span class="badge warn">recorded ' + esc(m.rules_passed) + " / " + esc(m.rules_total) + " — re-check in the browser differs; the recorded Rust score is authoritative</span>"
        : "") +
      '<span class="types">' +
      Object.keys(byType)
        .sort()
        .map(function (t) {
          return esc(t) + " " + byType[t].p + "/" + byType[t].n;
        })
        .join(" · ") +
      "</span></div>" +
      '<div class="seg" role="group" aria-label="Show">' +
      '<button type="button" data-rf="fail" aria-pressed="' + (filter === "fail") + '">Failing ' + failing + "</button>" +
      '<button type="button" data-rf="pass" aria-pressed="' + (filter === "pass") + '">Passing ' + scored.passed + "</button>" +
      '<button type="button" data-rf="all" aria-pressed="' + (filter === "all") + '">All ' + scored.total + "</button></div>" +
      (items.length
        ? '<ol class="checklist">' +
          items
            .slice(0, limit)
            .map(function (it) {
              return (
                '<li class="' + (it.r.ok ? "pass" : "fail") + '" value="' + (it.i + 1) + '">' +
                '<span class="verdict">' + (it.r.ok ? "✓ pass" : "✗ fail") + "</span>" +
                '<span class="rule-type">' + esc(it.rule.type) + "</span>" +
                '<div class="rule-body">' + ruleExpected(it.rule, it.r) +
                (!it.r.ok && it.r.detail ? '<div class="why">' + esc(it.r.detail) + "</div>" : "") +
                "</div></li>"
              );
            })
            .join("") +
          "</ol>" +
          (items.length > limit ? '<button type="button" class="btn more-rules">Show ' + Math.min(RULE_PAGE, items.length - limit) + " more of " + (items.length - limit) + "</button>" : "")
        : '<p class="empty">No ' + (filter === "fail" ? "failing" : filter === "pass" ? "passing" : "") + " assertions.</p>")
    );
  }

  function viewInspector(runInfo, slug, docId, params) {
    setNav("documents", runInfo.run_id);
    return Promise.all([getRun(runInfo.run_id), getManifest(runInfo.dataset.name)]).then(function (loaded) {
      var run = loaded[0];
      var manifest = loaded[1];
      var datasetName = runInfo.dataset.name;
      var entry = null;
      (manifest.documents || []).forEach(function (doc) {
        if (doc.id === docId) entry = doc;
      });
      if (!entry) throw new Error("Document " + docId + " is not in dataset " + datasetName);

      var models = run.models.slice().sort(function (a, b) {
        return (b.stats.head.value || 0) - (a.stats.head.value || 0);
      });
      var model = null;
      models.forEach(function (candidate) {
        if (slugOf(candidate.model) === slug || candidate.model === slug) model = candidate;
      });
      var canonical = function (m, values) {
        var next = {
          tab: params.get("tab"),
          diff: params.get("diff"),
          ov: params.get("ov"),
          page: params.get("page"),
          src: params.get("src"),
          cat: params.get("cat"),
          q: params.get("q"),
          rf: params.get("rf"),
        };
        Object.keys(values || {}).forEach(function (k) {
          next[k] = values[k];
        });
        return docHref(run.run_id, m.model, docId, next);
      };
      if (!model) {
        go(canonical(models[0]), true);
        return null;
      }

      var byDoc = recordsByDoc(run)[docId] || {};
      var rec = byDoc[model.model];
      var rulesDoc = isRulesDoc(entry);
      var inventory = (runInfo.outputs || {})[slugOf(model.model)] || { json: [], missing: [] };
      var list = docList(run, manifest, params);
      var pos = -1;
      list.forEach(function (d, i) {
        if (d.id === docId) pos = i;
      });
      var filters = { src: params.get("src"), cat: params.get("cat"), q: params.get("q") };
      var tabs = rulesDoc
        ? [["rules", "Rule checklist"], ["compare", "All models"], ["output", "Output"]]
        : [["diff", "Diff vs truth"], ["compare", "All models"], ["truth", "Ground truth"], ["output", "Output"]];
      var tab = params.get("tab");
      if (!tabs.some(function (t) { return t[0] === tab; })) tab = tabs[0][0];
      var diffMode = params.get("diff") === "unified" ? "unified" : "split";
      var ovOn = params.get("ov") !== "0";
      var page = parseInt(params.get("page") || "1", 10) || 1;
      var ruleFilter = params.get("rf") || "fail";

      var datasetBase = url("data/datasets/" + datasetName + "/");
      var truthUrl = entry.truth ? joinPath(datasetBase, entry.truth) : null;
      var rulesUrl = entry.rules ? joinPath(datasetBase, entry.rules) : null;
      var inputUrl = joinPath(datasetBase, entry.file);
      var previewUrl = entry.preview ? joinPath(datasetBase, entry.preview) : null;
      var isPdf = /\.pdf$/i.test(entry.file || "");
      function predUrl(m) {
        return url("data/outputs/" + run.run_id + "/" + slugOf(m.model) + "/" + docId + ".md");
      }
      function hasPred(m) {
        var inv = (runInfo.outputs || {})[slugOf(m.model)];
        return !inv || inv.missing.indexOf(docId) === -1;
      }
      function loadPred(m) {
        return hasPred(m) ? getText(predUrl(m)).catch(function () { return null; }) : Promise.resolve(null);
      }
      var jsonUrl = url("data/outputs/" + run.run_id + "/" + slugOf(model.model) + "/" + docId + ".json");
      var hasJson = inventory.json.indexOf(docId) !== -1;
      var anyJson = Object.keys(runInfo.outputs || {}).some(function (k) {
        return (runInfo.outputs[k].json || []).length;
      });

      var needAll = tab === "compare";
      return Promise.all([
        truthUrl ? getText(truthUrl).catch(function () { return null; }) : Promise.resolve(null),
        loadPred(model),
        rulesUrl ? getRules(rulesUrl) : Promise.resolve(null),
        hasJson ? getJSON(jsonUrl).catch(function () { return null; }) : Promise.resolve(null),
        needAll ? Promise.all(models.map(loadPred)) : Promise.resolve(null),
      ]).then(function (got) {
        var truth = got[0];
        var pred = got[1];
        var ruleList = got[2];
        var unified = got[3];
        var allPreds = got[4];
        var opts = run.normalize || {};
        document.title = docId + " · " + model.model + " · LiteOCR Benchmark";

        /* ---- model chips */
        var chips = models
          .map(function (m, i) {
            var r = byDoc[m.model];
            var s = docScore(r);
            return (
              '<a class="chip' + (m === model ? " on" : "") + '" href="' + esc(canonical(m)) + '"' +
              (m === model ? ' aria-current="true"' : "") + ' title="' + esc(m.model + (r && r.error ? " — failed: " + r.error : "")) + '">' +
              "<code>" + esc(m.model) + "</code>" +
              '<span class="chip-s' + (r && r.error ? " fail" : "") + '">' + (r && r.error ? "failed" : esc(fixed(s, 1))) + "</span></a>"
            );
          })
          .join("");

        /* ---- analysis body */
        var body = "";
        var stats = "";
        if (tab === "diff") {
          if (pred === null) {
            body = '<p class="empty">No saved output for <code>' + esc(model.model) + "</code> on this document" +
              (rec && rec.error ? " — the call failed: " + esc(rec.error) : "") + ".</p>";
          } else if (truth === null) {
            body = '<p class="empty">The ground truth could not be loaded.</p>';
          } else {
            var d = wordDiff(truth, pred);
            stats = d.eq + " words match · <del>" + d.del + " missing</del> · <ins>" + d.ins + " extra</ins>";
            body =
              '<div class="bar"><div class="seg" role="group" aria-label="Diff layout">' +
              '<button type="button" data-diff="split" aria-pressed="' + (diffMode === "split") + '">Side by side</button>' +
              '<button type="button" data-diff="unified" aria-pressed="' + (diffMode === "unified") + '">Unified</button></div>' +
              '<p class="hint">' + stats + "</p></div>" +
              '<div class="diff-legend"><span><span class="swatch del"></span>in the truth, missing from the output</span>' +
              '<span><span class="swatch ins"></span>in the output, not in the truth</span>' +
              "<span>word-level LCS, same case/markdown normalisation as the scorer</span></div>" +
              (rec && rec.table_only
                ? '<p class="note">Table-only document: the score is the <strong>table score</strong> (table rows only); the diff shows the whole text.</p>'
                : "") +
              (diffMode === "unified"
                ? '<div class="pane tall" tabindex="0" role="region" aria-label="Unified word diff">' + renderUnified(d) + "</div>"
                : '<div class="grid-2 tight"><div><div class="pane-head"><h3>Ground truth</h3></div>' +
                  '<div class="pane tall" tabindex="0" role="region" aria-label="Ground truth, missing words marked">' + renderSide(d.t, d.tMarks, "del") + "</div></div>" +
                  '<div><div class="pane-head"><h3>' + esc(model.model) + "</h3></div>" +
                  '<div class="pane tall" tabindex="0" role="region" aria-label="Model output, extra words marked">' + renderSide(d.p, d.pMarks, "ins") + "</div></div></div>");
          }
        } else if (tab === "rules") {
          var scored = pred !== null && ruleList ? scoreRules(pred, ruleList, opts) : null;
          body = rulesChecklist(ruleList, scored, rec, ruleFilter, RULE_PAGE);
        } else if (tab === "compare") {
          body =
            '<p class="hint">Every model on this document. ' +
            (rulesDoc ? "Pass counts are re-checked in your browser." : "Highlighted words are not in the ground truth.") +
            "</p>" +
            '<div class="compare">' +
            models
              .map(function (m, i) {
                var p = allPreds ? allPreds[i] : null;
                var r = byDoc[m.model];
                var inner;
                var sub = "";
                if (p === null) inner = '<p class="empty">' + (r && r.error ? "Failed: " + esc(r.error) : "No saved output.") + "</p>";
                else if (rulesDoc) {
                  if (ruleList) {
                    var sc = scoreRules(p, ruleList, opts);
                    sub = sc.passed + " / " + sc.total + " rules";
                  }
                  inner = esc(p);
                } else if (truth !== null) {
                  var dd = wordDiff(truth, p);
                  sub = dd.del + " missing · " + dd.ins + " extra";
                  inner = renderSide(dd.p, dd.pMarks, "ins");
                } else inner = esc(p);
                return (
                  '<div class="col' + (m === model ? " on" : "") + '"><div class="col-head"><a href="' + esc(canonical(m, { tab: "compare" })) + '"><code>' +
                  esc(m.model) + "</code></a>" +
                  '<span class="col-s">' + esc(fixed(docScore(r), 1)) + "</span></div>" +
                  (sub ? '<div class="col-sub hint">' + esc(sub) + "</div>" : "") +
                  (p === null ? inner : '<div class="pane tall" tabindex="0" role="region" aria-label="' + esc(m.model) + ' output">' + inner + "</div>") +
                  "</div>"
                );
              })
              .join("") +
            "</div>";
        } else if (tab === "truth") {
          body =
            truth === null
              ? '<p class="empty">The ground truth could not be loaded.</p>'
              : '<div class="pane-head"><h3>Ground truth</h3><a class="hint" href="' + esc(truthUrl) + '" rel="noopener">' + esc(pathLabel(entry.truth)) + "</a></div>" +
                '<div class="pane tall" tabindex="0" role="region" aria-label="Ground truth markdown source">' + esc(truth) + "</div>" +
                '<p class="hint">Markdown <em>source</em> — the exact text the metrics compare against.</p>';
        } else {
          body =
            pred === null
              ? '<p class="empty">No saved output' + (rec && rec.error ? " — the call failed: " + esc(rec.error) : "") + ".</p>"
              : '<div class="pane-head"><h3>' + esc(model.model) + '</h3><a class="hint" href="' + esc(predUrl(model)) + '" rel="noopener">raw .md</a></div>' +
                '<div class="pane tall" tabindex="0" role="region" aria-label="Model output">' + esc(pred) + "</div>";
        }

        /* ---- verify commands */
        var repoBase = "benchmark/datasets/" + datasetName + "/";
        var predFile = docId.replace(/\//g, "_") + ".pred.md";
        var parseCommand = "liteocr parse " + joinPath(repoBase, entry.file) + " -m " + model.model + " > " + predFile;
        var scoreCommand = rulesDoc
          ? "liteocr bench run --dataset benchmark/datasets/" + datasetName + " \\\n    --models " + model.model + " --filter " + docId
          : "liteocr bench score " + predFile + " " + joinPath(repoBase, entry.truth);

        var prev = pos > 0 ? list[pos - 1] : null;
        var next = pos >= 0 && pos < list.length - 1 ? list[pos + 1] : null;
        function docLink(d) {
          return docHref(run.run_id, model.model, d.id, { tab: params.get("tab"), diff: params.get("diff"), ov: params.get("ov"), src: filters.src, cat: filters.cat, q: filters.q });
        }

        var html =
          '<nav class="crumbs" aria-label="Breadcrumb"><a href="' + esc(listHref("documents", run.run_id, filters)) + '">Documents</a>' +
          (filters.src ? " / " + esc(filters.src) : "") + (filters.cat ? " / " + esc(filters.cat) : "") + "</nav>" +
          '<div class="doc-head"><h1>' + esc(docId) + "</h1>" +
          '<div class="doc-nav">' +
          (prev ? '<a class="btn sm" href="' + esc(docLink(prev)) + '" title="Previous document (k)">‹ Prev</a>' : '<span class="btn sm" aria-disabled="true">‹ Prev</span>') +
          '<span class="pos">' + (pos + 1) + " / " + list.length + "</span>" +
          (next ? '<a class="btn sm" href="' + esc(docLink(next)) + '" title="Next document (j)">Next ›</a>' : '<span class="btn sm" aria-disabled="true">Next ›</span>') +
          '<button type="button" class="btn sm" id="copy-link" title="Copy a link to exactly this view">Copy link</button>' +
          "</div></div>" +
          '<ul class="badges">' +
          '<li class="badge strong">' + esc(entry.category) + "</li>" +
          '<li class="badge">' + esc(entry.pages) + (entry.pages === 1 ? " page" : " pages") + "</li>" +
          (rulesDoc ? '<li class="badge strong">rule-scored</li>' : "") +
          (rec && rec.table_only ? '<li class="badge strong">table-only</li>' : "") +
          (entry.tags || []).map(function (t) { return '<li class="badge">' + esc(t) + "</li>"; }).join("") +
          '<li class="badge">' + esc(datasetName) + " v" + esc(manifest.version) + "</li>" +
          (entry.license ? '<li class="badge">' + esc(entry.license) + "</li>" : "") +
          "</ul>" +
          (entry.attribution ? '<p class="hint attr">' + esc(entry.attribution) + "</p>" : "") +
          '<div class="chips" role="navigation" aria-label="Models (m / M to cycle)">' + chips + "</div>" +
          '<div class="inspector">' +
          '<section class="panel source" aria-label="Input document"><div class="panel-title"><h2>Input</h2><span class="hint">' +
          esc(pathLabel(entry.file)) + "</span></div><div id=\"sv-slot\"></div></section>" +
          '<section class="panel analysis" aria-label="Analysis">' +
          metricTiles(rec) +
          (rec && rec.error ? '<p class="note fail">This call failed: ' + esc(rec.error) + "</p>" : "") +
          '<div class="tabs" role="tablist">' +
          tabs
            .map(function (t, i) {
              return '<a role="tab" class="tab" href="' + esc(canonical(model, { tab: t[0] })) + '" aria-selected="' + (t[0] === tab) + '" title="' + t[1] + " (" + (i + 1) + ')">' + esc(t[1]) + "</a>";
            })
            .join("") +
          "</div>" +
          '<div class="tab-body" role="tabpanel">' + body + "</div>" +
          "</section></div>" +
          '<section class="panel"><h2>Verify it yourself</h2>' +
          "<p>" + (rulesDoc ? "Parse the page with the same model, then re-check the assertions (<code>bench score</code> compares transcripts only):" : "Parse the same document with the same model and score it locally:") + "</p>" +
          codeBlock(parseCommand) + codeBlock(scoreCommand) +
          '<p class="hint">Served from this site: ' +
          (rulesDoc ? (rulesUrl ? '<a href="' + esc(rulesUrl) + '" rel="noopener">assertions</a>' : "no assertion file") : truthUrl ? '<a href="' + esc(truthUrl) + '" rel="noopener">ground truth</a>' : "no ground truth") +
          (pred !== null ? ' · <a href="' + esc(predUrl(model)) + '" rel="noopener">model output</a>' : "") +
          (hasJson ? ' · <a href="' + esc(jsonUrl) + '" rel="noopener">unified JSON</a>' : "") +
          ' · <a href="' + esc(inputUrl) + '" rel="noopener">input</a> · <a href="' + esc(url("data/runs/" + run.run_id + ".json")) + '" rel="noopener">run JSON</a>.</p>' +
          "</section>";

        view.innerHTML = html;
        var chipRow = view.querySelector(".chips");
        var onChip = chipRow.querySelector(".chip.on");
        if (onChip && onChip.offsetLeft + onChip.offsetWidth > chipRow.clientWidth) {
          chipRow.scrollLeft = onChip.offsetLeft - chipRow.offsetLeft - 16;
        }

        /* ---- source viewer: reuse across model / tab switches */
        var svKey = run.run_id + "|" + docId;
        if (!SV || SV.key !== svKey) {
          SV = new SourceViewer(svKey, {
            docId: docId,
            inputUrl: inputUrl,
            previewUrl: previewUrl,
            imageUrl: isPdf ? null : previewUrl || inputUrl,
            isPdf: isPdf,
            pages: entry.pages || 1,
          });
        }
        document.getElementById("sv-slot").appendChild(SV.el);
        SV.onPage = function (n) {
          go(canonical(model, { page: n > 1 ? String(n) : null }), true);
        };
        SV.onOverlay = function (on) {
          go(canonical(model, { ov: on ? null : "0" }), true);
        };
        SV.overlayOn = ovOn;
        if (page !== SV.page) SV.showPage(page);
        var overlay = unified ? blocksByPage(unified) : null;
        var message;
        if (overlay && overlay.count) message = "Layout boxes: <code>" + esc(model.model) + "</code>, " + overlay.count + " blocks from the unified response.";
        else if (overlay) message = "The unified response of <code>" + esc(model.model) + "</code> has no block bounding boxes.";
        else if (anyJson) message = "No unified JSON was saved for <code>" + esc(model.model) + "</code> on this document, so there are no layout boxes.";
        else message = "Layout boxes need the unified JSON response; this run saved markdown only.";
        SV.setOverlay(overlay, message);

        /* ---- wiring */
        wireCopyButtons(view);
        view.querySelector("#copy-link").addEventListener("click", function (e) {
          copyText(location.href, e.currentTarget, "Copy link");
        });
        view.querySelectorAll("button[data-diff]").forEach(function (b) {
          b.addEventListener("click", function () {
            go(canonical(model, { diff: b.getAttribute("data-diff") === "split" ? null : "unified" }), true);
          });
        });
        if (tab === "rules") wireRuleBody();
        function wireRuleBody() {
          view.querySelectorAll(".tab-body button[data-rf]").forEach(function (b) {
            b.addEventListener("click", function () {
              var target = b.getAttribute("data-rf");
              go(canonical(model, { rf: target === "fail" ? null : target }), true);
            });
          });
          var again = view.querySelector(".more-rules");
          if (again)
            again.addEventListener("click", function () {
              var n = view.querySelectorAll(".checklist > li").length + RULE_PAGE;
              view.querySelector(".tab-body").innerHTML = rulesChecklist(ruleList, scoreRules(pred, ruleList, opts), rec, ruleFilter, n);
              wireRuleBody();
            });
        }

        var idx = models.indexOf(model);
        KEYS = {
          j: function () {
            if (next) go(docLink(next));
          },
          k: function () {
            if (prev) go(docLink(prev));
          },
          m: function () {
            go(canonical(models[(idx + 1) % models.length]), true);
          },
          M: function () {
            go(canonical(models[(idx - 1 + models.length) % models.length]), true);
          },
          d: function () {
            if (tab === "diff") go(canonical(model, { diff: diffMode === "split" ? "unified" : null }), true);
          },
          o: function () {
            if (SV.overlay && SV.overlay.count) SV.onOverlay(!SV.overlayOn);
          },
          "[": function () {
            SV.onPage(SV.page - 1);
          },
          "]": function () {
            SV.onPage(SV.page + 1);
          },
        };
        tabs.forEach(function (t, i) {
          KEYS[String(i + 1)] = function () {
            go(canonical(model, { tab: t[0] }), true);
          };
        });
        return "doc|" + run.run_id + "|" + docId;
      });
    });
  }

  /* ------------------------------------------------------------------ boot */

  function render() {
    var route = parseHash();
    var parts = route.parts;
    var params = route.params;
    var name = parts[0] || "leaderboard";
    view.setAttribute("aria-busy", "true");
    var task;
    try {
      if (name === "doc") {
        // Old `#/doc/<id>?run=&model=` links: rewrite to the canonical `#/<run>/<model>/<doc>`.
        var legacyRun = resolveRun(params);
        var legacyModel = params.get("model") || ((legacyRun.models || [])[0] || {}).model || "";
        go(docHref(legacyRun.run_id, legacyModel, parts.slice(1).join("/"), { diff: params.get("diff") }), true);
        return;
      }
      var direct = findRun(parts[0]);
      if (direct && parts.length >= 3) task = viewInspector(direct, parts[1], parts.slice(2).join("/"), params);
      else if (direct) task = viewLeaderboard(direct, params);
      else if (name === "documents") task = viewDocuments(resolveRun(params), params);
      else task = viewLeaderboard(resolveRun(params), params);
    } catch (err) {
      showError(err);
      return;
    }
    Promise.resolve(task).then(
      function (scrollKey) {
        if (scrollKey === null) return; // redirected
        view.removeAttribute("aria-busy");
        if (scrollKey !== lastScrollKey) {
          window.scrollTo(0, 0);
          lastScrollKey = scrollKey;
        }
      },
      function (err) {
        showError(err);
      }
    );
  }

  var pendingG = false;
  function onKey(e) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    var t = e.target;
    var tag = t && t.tagName;
    if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA" || (t && t.isContentEditable)) return;
    var help = document.getElementById("help");
    if (e.key === "?") {
      if (help && help.showModal && !help.open) help.showModal();
      e.preventDefault();
      return;
    }
    if (help && help.open) return;
    if (pendingG) {
      pendingG = false;
      var run = parseHash().params.get("run");
      var direct = findRun(parseHash().parts[0]);
      var runId = direct ? direct.run_id : run;
      if (e.key === "l") go(listHref("leaderboard", runId));
      else if (e.key === "d") go(listHref("documents", runId));
      return;
    }
    if (e.key === "g") {
      pendingG = true;
      setTimeout(function () {
        pendingG = false;
      }, 1200);
      return;
    }
    var fn = KEYS[e.key];
    if (fn) {
      e.preventDefault();
      fn();
    }
  }

  function wireChrome() {
    var toggle = document.getElementById("theme");
    if (toggle)
      toggle.addEventListener("click", function () {
        var root = document.documentElement;
        var dark = root.getAttribute("data-theme") === "dark" || (!root.getAttribute("data-theme") && window.matchMedia("(prefers-color-scheme: dark)").matches);
        var next = dark ? "light" : "dark";
        root.setAttribute("data-theme", next);
        try {
          localStorage.setItem("liteocr-theme", next);
        } catch (e) {
          /* private mode */
        }
      });
    var helpBtn = document.getElementById("help-btn");
    var help = document.getElementById("help");
    if (helpBtn && help && help.showModal)
      helpBtn.addEventListener("click", function () {
        help.showModal();
      });
    else if (helpBtn) helpBtn.hidden = true;
    if (HOME) document.getElementById("brand").setAttribute("href", HOME);
    var docs = document.getElementById("nav-docs");
    if (DOCS && docs) {
      docs.setAttribute("href", DOCS);
      docs.hidden = false;
    }
    document.addEventListener("keydown", onKey);
    var resizeTimer = null;
    window.addEventListener("resize", function () {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(function () {
        if (SV && SV.pdf && document.body.contains(SV.el)) SV.renderPdfPage();
      }, 250);
    });
  }

  function boot() {
    wireChrome();
    getJSON(url("data/index.json")).then(
      function (data) {
        INDEX = data;
        var first = (data.runs || [])[0];
        var dataset = first ? (data.datasets || {})[first.dataset.name] : null;
        if (dataset) {
          var nameEl = document.getElementById("footer-dataset");
          var licenseEl = document.getElementById("footer-license");
          if (nameEl) nameEl.textContent = dataset.name + " v" + dataset.version;
          if (licenseEl) licenseEl.textContent = dataset.license || "see the repository";
          var truthEl = document.getElementById("footer-truth");
          if (truthEl && (dataset.sources || (dataset.kinds || {}).rules)) {
            truthEl.textContent =
              "Each document keeps the ground truth its own source ships: an exact transcript, or " +
              "machine-checkable assertions for pages that have no reference transcript.";
          }
          var attrEl = document.getElementById("footer-attribution");
          if (attrEl && dataset.attribution) {
            attrEl.textContent = dataset.attribution;
            attrEl.hidden = false;
          }
        }
        var generated = document.getElementById("footer-generated");
        if (generated && data.generated_at) generated.textContent = "site built " + data.generated_at;
        window.addEventListener("hashchange", render);
        render();
      },
      function (err) {
        showError(err);
      }
    );
  }

  boot();
})();
