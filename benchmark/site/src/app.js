/* LiteOCR benchmark results viewer.
 *
 * Vanilla ES2018, no framework, no external requests: everything is fetched with
 * relative URLs from the `data/` directory written by benchmark/site/build.py.
 *
 * Routes (hash based):
 *   #/leaderboard?run=<run_id>&sort=<col>&dir=asc|desc
 *   #/documents?run=<run_id>&cat=<category>&q=<text>&sort=<col>&dir=asc|desc
 *   #/doc/<doc_id>?run=<run_id>&model=<model>&diff=split|unified
 */
(function () {
  "use strict";

  var REPO = "https://github.com/ajinkyashejul/liteocr";
  var METHODOLOGY = REPO + "/blob/main/benchmark/README.md";
  var DIFF_CELL_CAP = 6000000; // LCS table cells we are willing to allocate

  var INDEX = null;
  var runCache = {};
  var textCache = {};
  var manifestCache = {};
  var lastPath = null;

  var view = document.getElementById("view");

  /* ------------------------------------------------------------------ utils */

  function esc(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function fixed(value, digits) {
    return typeof value === "number" && isFinite(value) ? value.toFixed(digits) : "—";
  }

  function msFmt(value) {
    return typeof value === "number" && isFinite(value) ? Math.round(value) + " ms" : "—";
  }

  function moneyFmt(value) {
    return typeof value === "number" && isFinite(value) ? "$" + value.toFixed(2) : "—";
  }

  function slugOf(model) {
    return String(model).replace(/\//g, "_");
  }

  function getJSON(url) {
    return fetch(url, { cache: "no-cache" }).then(function (resp) {
      if (!resp.ok) throw new Error("Could not load " + url + " (HTTP " + resp.status + ")");
      return resp.json();
    });
  }

  function getText(url) {
    if (textCache[url]) return Promise.resolve(textCache[url]);
    return fetch(url, { cache: "no-cache" }).then(function (resp) {
      if (!resp.ok) throw new Error("Could not load " + url + " (HTTP " + resp.status + ")");
      return resp.text().then(function (text) {
        textCache[url] = text;
        return text;
      });
    });
  }

  function getRun(runId) {
    if (runCache[runId]) return Promise.resolve(runCache[runId]);
    return getJSON("data/runs/" + encodeURIComponent(runId) + ".json").then(function (data) {
      runCache[runId] = data;
      return data;
    });
  }

  function getManifest(name) {
    if (manifestCache[name]) return Promise.resolve(manifestCache[name]);
    return getJSON("data/datasets/" + encodeURIComponent(name) + "/manifest.json").then(function (data) {
      manifestCache[name] = data;
      return data;
    });
  }

  /* ----------------------------------------------------------------- router */

  function parseHash() {
    var raw = location.hash.replace(/^#/, "") || "/leaderboard";
    var split = raw.indexOf("?");
    var pathPart = split === -1 ? raw : raw.slice(0, split);
    var queryPart = split === -1 ? "" : raw.slice(split + 1);
    return {
      parts: pathPart.split("/").filter(Boolean).map(decodeURIComponent),
      params: new URLSearchParams(queryPart),
    };
  }

  /** Build a hash href, carrying `run` over unless it is overridden. */
  function href(path, overrides) {
    var current = parseHash().params;
    var next = new URLSearchParams();
    if (current.get("run")) next.set("run", current.get("run"));
    Object.keys(overrides || {}).forEach(function (key) {
      var value = overrides[key];
      if (value === null || value === undefined || value === "") next.delete(key);
      else next.set(key, value);
    });
    var query = next.toString();
    return "#" + path + (query ? "?" + query : "");
  }

  function go(path, overrides) {
    location.hash = href(path, overrides);
  }

  function setNav(active) {
    ["leaderboard", "documents"].forEach(function (name) {
      var link = document.getElementById("nav-" + name);
      if (!link) return;
      link.setAttribute("href", href("/" + name, {}));
      if (name === active) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }

  function resolveRun(params) {
    var wanted = params.get("run");
    var runs = INDEX.runs || [];
    for (var i = 0; i < runs.length; i++) {
      if (runs[i].run_id === wanted) return runs[i];
    }
    return runs[0]; // newest first
  }

  function showError(err) {
    view.removeAttribute("aria-busy");
    view.innerHTML =
      '<div class="error"><h2>Something went wrong</h2><p>' +
      esc(err && err.message ? err.message : err) +
      "</p><p class=\"hint\">This page is static: it reads the JSON and markdown written by " +
      "<code>benchmark/site/build.py</code>. Serve <code>dist/</code> over HTTP (for example " +
      "<code>python -m http.server -d benchmark/site/dist 8000</code>) rather than opening the " +
      "file directly.</p></div>";
  }

  /* ------------------------------------------------------- shared fragments */

  function runSelector(run) {
    var options = (INDEX.runs || [])
      .map(function (item) {
        var label =
          item.run_id +
          " · " +
          (item.dataset.name || "?") +
          " v" +
          (item.dataset.version || "?") +
          " · " +
          (item.created_at || "").slice(0, 10);
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
      '<div class="field"><label for="run-select">Run</label>' +
      '<select id="run-select">' +
      options +
      "</select></div>"
    );
  }

  function runMeta(run) {
    var sha = (run.dataset.sha256 || "").slice(0, 12);
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
      "</span>" +
      "<span>" +
      esc(run.dataset.documents) +
      " documents</span>" +
      "<span>" +
      esc((run.models || []).length) +
      " models</span>" +
      "<span>LiteOCR " +
      esc(run.liteocr_version) +
      "</span>" +
      "<span>Run " +
      esc(run.created_at) +
      "</span>" +
      '<span title="SHA-256 of the manifest, every input and every truth file">dataset sha256 <code>' +
      esc(sha) +
      "…</code></span>" +
      (flags.length ? "<span>normalisation: " + esc(flags.join(", ")) + "</span>" : "") +
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

  function wireCopyButtons(root) {
    root.querySelectorAll(".copy-btn").forEach(function (button) {
      button.addEventListener("click", function () {
        var text = button.getAttribute("data-copy") || "";
        var done = function () {
          button.textContent = "Copied";
          setTimeout(function () {
            button.textContent = "Copy";
          }, 1400);
        };
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text).then(done, function () {
            button.textContent = "Press ⌘/Ctrl+C";
          });
        } else {
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
        var arrow = isSorted ? (sortDir === "asc" ? " ▲" : " ▼") : "";
        var attrs =
          (col.num ? ' class="num"' : "") +
          (isSorted ? ' aria-sort="' + (sortDir === "asc" ? "ascending" : "descending") + '"' : "");
        if (!col.value) {
          return "<th scope=\"col\"" + attrs + "><span class='sort-btn'>" + esc(col.label) + "</span></th>";
        }
        return (
          '<th scope="col"' +
          attrs +
          '><button type="button" class="sort-btn" data-sort="' +
          esc(col.key) +
          '" title="' +
          esc(col.title || "Sort by " + col.label) +
          '">' +
          esc(col.label) +
          '<span class="sort-arrow" aria-hidden="true">' +
          arrow +
          "</span></button></th>"
        );
      })
      .join("");

    var body = rows
      .map(function (row, position) {
        var cells = columns
          .map(function (col) {
            var cell = col.cell(row, position);
            var cls = col.num ? "num " + (cell.cls || "") : cell.cls || "";
            return (
              "<td" +
              (cls ? ' class="' + esc(cls.trim()) + '"' : "") +
              (cell.style ? ' style="' + esc(cell.style) + '"' : "") +
              (cell.title ? ' title="' + esc(cell.title) + '"' : "") +
              ">" +
              cell.html +
              "</td>"
            );
          })
          .join("");
        return "<tr" + (options.rowClass ? ' class="' + esc(options.rowClass(row, position)) + '"' : "") + ">" + cells + "</tr>";
      })
      .join("");

    return (
      '<div class="table-wrap"><table>' +
      (options.caption ? "<caption>" + options.caption + "</caption>" : "") +
      "<thead><tr>" +
      head +
      "</tr></thead><tbody>" +
      (body || '<tr><td colspan="' + columns.length + '" class="empty">No rows</td></tr>') +
      "</tbody></table></div>"
    );
  }

  function wireSorting(root, path) {
    root.querySelectorAll("button[data-sort]").forEach(function (button) {
      button.addEventListener("click", function () {
        var key = button.getAttribute("data-sort");
        var params = parseHash().params;
        var dir = params.get("sort") === key && params.get("dir") === "desc" ? "asc" : "desc";
        // Text columns read better ascending first.
        if (params.get("sort") !== key && button.getAttribute("data-asc") === "1") dir = "asc";
        var overrides = { sort: key, dir: dir };
        ["cat", "q", "model", "diff"].forEach(function (name) {
          if (params.get(name)) overrides[name] = params.get(name);
        });
        go(path, overrides);
      });
    });
  }

  /* --------------------------------------------------------- heatmap colour */

  function heatScale(values) {
    var clean = values.filter(function (value) {
      return typeof value === "number" && isFinite(value);
    });
    var lo = clean.length ? Math.min.apply(null, clean) : 0;
    var hi = clean.length ? Math.max.apply(null, clean) : 100;
    if (hi - lo < 5) lo = hi - 5; // do not amplify sub-point noise into a full gradient
    return function (value) {
      if (typeof value !== "number" || !isFinite(value)) return null;
      var t = (value - lo) / (hi - lo || 1);
      t = Math.max(0, Math.min(1, t));
      return Math.round(t * 140); // 0 = red, 140 = green
    };
  }

  /* ------------------------------------------------------------ leaderboard */

  var LEADERBOARD_COLUMNS = [
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
      value: function (row) {
        return row.model;
      },
      asc: true,
      cell: function (row) {
        return { html: "<code>" + esc(row.model) + "</code>", cls: "model" };
      },
    },
    {
      key: "overall",
      label: "Overall",
      num: true,
      title: "100 × mean(char_similarity); a failed call scores 0",
      value: function (row) {
        return row.summary.overall;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.overall, 2)), cls: "primary" };
      },
    },
    {
      key: "char_similarity",
      label: "Char sim",
      num: true,
      title: "1 − levenshtein / max(len)",
      value: function (row) {
        return row.summary.char_similarity;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.char_similarity, 3)) };
      },
    },
    {
      key: "cer",
      label: "CER",
      num: true,
      title: "Character error rate (lower is better)",
      value: function (row) {
        return row.summary.cer;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.cer, 4)) };
      },
    },
    {
      key: "wer",
      label: "WER",
      num: true,
      title: "Word error rate (lower is better)",
      value: function (row) {
        return row.summary.wer;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.wer, 4)) };
      },
    },
    {
      key: "word_f1",
      label: "Word F1",
      num: true,
      value: function (row) {
        return row.summary.word_f1;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.word_f1, 3)) };
      },
    },
    {
      key: "order_score",
      label: "Order",
      num: true,
      title: "Kendall-τ-style reading-order agreement",
      value: function (row) {
        return row.summary.order_score;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.order_score, 3)) };
      },
    },
    {
      key: "table_score",
      label: "Table",
      num: true,
      title: "Character similarity restricted to markdown table rows",
      value: function (row) {
        return row.summary.table_score;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.table_score, 3)) };
      },
    },
    {
      key: "latency_p50_ms",
      label: "p50",
      num: true,
      title: "Median client-measured latency",
      value: function (row) {
        return row.summary.latency_p50_ms;
      },
      cell: function (row) {
        return { html: esc(msFmt(row.summary.latency_p50_ms)) };
      },
    },
    {
      key: "latency_p95_ms",
      label: "p95",
      num: true,
      value: function (row) {
        return row.summary.latency_p95_ms;
      },
      cell: function (row) {
        return { html: esc(msFmt(row.summary.latency_p95_ms)) };
      },
    },
    {
      key: "latency_per_page_ms",
      label: "ms/page",
      num: true,
      value: function (row) {
        return row.summary.latency_per_page_ms;
      },
      cell: function (row) {
        return { html: esc(fixed(row.summary.latency_per_page_ms, 0)) };
      },
    },
    {
      key: "cost_per_1k_pages_usd",
      label: "$/1k pages",
      num: true,
      title: "Public list price per 1,000 pages",
      value: function (row) {
        return row.summary.cost_per_1k_pages_usd;
      },
      cell: function (row) {
        return { html: esc(moneyFmt(row.summary.cost_per_1k_pages_usd)) };
      },
    },
    {
      key: "failed",
      label: "Failed",
      num: true,
      value: function (row) {
        return row.summary.failed;
      },
      cell: function (row) {
        var failed = row.summary.failed || 0;
        return {
          html: esc(failed + "/" + row.summary.documents),
          cls: failed ? "fail" : "muted",
        };
      },
    },
  ];

  function categoryHeatmap(run) {
    var categories = run.categories || [];
    if (!categories.length) return "";
    var values = [];
    run.models.forEach(function (model) {
      categories.forEach(function (category) {
        var bucket = (model.summary.by_category || {})[category];
        if (bucket) values.push(bucket.overall);
      });
    });
    var hue = heatScale(values);

    var head =
      '<th scope="col">Model</th>' +
      categories
        .map(function (category) {
          return '<th scope="col" class="num">' + esc(category) + "</th>";
        })
        .join("");

    var body = run.models
      .map(function (model) {
        var cells = categories
          .map(function (category) {
            var bucket = (model.summary.by_category || {})[category];
            if (!bucket) return '<td class="num muted">—</td>';
            var color = hue(bucket.overall);
            return (
              '<td class="heat" style="--hm:' +
              color +
              '" title="' +
              esc(
                model.model +
                  " · " +
                  category +
                  " · " +
                  bucket.documents +
                  " docs · overall " +
                  fixed(bucket.overall, 2)
              ) +
              '">' +
              esc(fixed(bucket.overall, 1)) +
              "</td>"
            );
          })
          .join("");
        return '<tr><th scope="row" class="model"><code>' + esc(model.model) + "</code></th>" + cells + "</tr>";
      })
      .join("");

    return (
      '<div class="table-wrap"><table><caption>Overall score per category. ' +
      "Colour is scaled across the whole table, green = best, red = worst.</caption>" +
      "<thead><tr>" +
      head +
      "</tr></thead><tbody>" +
      body +
      "</tbody></table></div>"
    );
  }

  function viewLeaderboard(params) {
    var run = resolveRun(params);
    if (!run) throw new Error("No benchmark runs found in data/index.json");
    document.title = "Leaderboard · LiteOCR Benchmark";

    var sortKey = params.get("sort") || "overall";
    var sortDir = params.get("dir") || (sortKey === "model" ? "asc" : "desc");
    if (["cer", "wer", "latency_p50_ms", "latency_p95_ms", "latency_per_page_ms", "cost_per_1k_pages_usd", "failed"].indexOf(sortKey) !== -1 && !params.get("dir")) {
      sortDir = "asc";
    }

    var reproduce =
      "liteocr bench run \\\n" +
      "    --dataset benchmark/datasets/" +
      run.dataset.name +
      " \\\n" +
      "    --models " +
      run.models
        .map(function (model) {
          return model.model;
        })
        .join(" ") +
      " \\\n" +
      "    --concurrency 4 --save-outputs benchmark/runs/outputs";

    var html =
      "<h1>Leaderboard</h1>" +
      '<div class="toolbar">' +
      runSelector(run) +
      '<p class="hint">Every number below comes from <code>' +
      esc(run.file) +
      "</code>, committed in the repository.</p>" +
      "</div>" +
      runMeta(run) +
      sortableTable({
        columns: LEADERBOARD_COLUMNS,
        rows: run.models,
        sortKey: sortKey,
        sortDir: sortDir,
        caption:
          "Models ranked on " +
          esc(run.dataset.name) +
          " v" +
          esc(run.dataset.version) +
          ". Click a column heading to sort. Higher is better for Overall, char similarity, " +
          "word F1, order and table; lower is better for CER, WER, latency and cost.",
        rowClass: function (row, position) {
          return position === 0 && sortKey === "overall" && sortDir === "desc" ? "top" : "";
        },
      }) +
      '<div class="panel-title"><h2>Score by category</h2>' +
      '<span class="heat-legend">worse <span class="heat-bar"></span> better</span></div>' +
      categoryHeatmap(run) +
      '<div class="grid-2">' +
      '<section class="panel"><h2>Methodology</h2>' +
      "<p>Scores are deterministic text comparisons — no LLM judge. Predictions and ground truth are " +
      "normalised (NFKC, markdown syntax stripped, quotes and dashes straightened, whitespace collapsed" +
      (run.normalize && run.normalize.case_insensitive ? ", lowercased" : "") +
      "), then compared:</p>" +
      "<ul><li><strong>Overall</strong> = 100 × mean character similarity, " +
      "<code>1 − levenshtein / max(len)</code>; a failed call scores 0.</li>" +
      "<li><strong>CER / WER</strong> are edit rates over characters and whitespace tokens.</li>" +
      "<li><strong>Order</strong> is Kendall-τ-style agreement on the order of lines present in both texts.</li>" +
      "<li><strong>Table</strong> is character similarity restricted to markdown table rows.</li>" +
      "<li><strong>Latency</strong> is measured from the client and includes upload, queueing and polling; " +
      "<strong>$/1k pages</strong> uses public list prices.</li></ul>" +
      '<p>Full definitions and caveats: <a href="' +
      METHODOLOGY +
      '" rel="noopener noreferrer">benchmark/README.md</a>. Every result file records the ' +
      "dataset SHA-256, the LiteOCR version and the normalisation options, so a run can be checked " +
      "against the exact bytes it scored.</p>" +
      "</section>" +
      '<section class="panel"><h2>Reproduce this run</h2>' +
      "<p>Install the CLI, set the provider keys you want to test, then:</p>" +
      codeBlock(reproduce) +
      "<p>Regenerate the dataset itself (byte-reproducible) and re-render the leaderboard:</p>" +
      codeBlock(
        "python benchmark/generate_synthetic.py\n" +
          "liteocr bench report benchmark/results/*.json > benchmark/LEADERBOARD.md"
      ) +
      '<p class="hint">Score a single prediction with no network access: ' +
      "<code>liteocr bench score prediction.md truth.md</code>. Open any document below to get the " +
      "exact commands for it.</p>" +
      "</section></div>";

    view.innerHTML = html;
    wireSorting(view, "/leaderboard");
    wireCopyButtons(view);
    wireRunSelect(view, "/leaderboard");
  }

  function wireRunSelect(root, path) {
    var select = root.querySelector("#run-select");
    if (!select) return;
    select.addEventListener("change", function () {
      go(path, { run: select.value, sort: null, dir: null, cat: null, q: null, model: null });
    });
  }

  /* -------------------------------------------------------------- documents */

  function viewDocuments(params) {
    var runInfo = resolveRun(params);
    if (!runInfo) throw new Error("No benchmark runs found in data/index.json");
    document.title = "Documents · LiteOCR Benchmark";

    return Promise.all([getRun(runInfo.run_id), getManifest(runInfo.dataset.name)]).then(function (loaded) {
      var run = loaded[0];
      var manifest = loaded[1];
      var category = params.get("cat") || "";
      var query = (params.get("q") || "").toLowerCase();

      var models = run.models
        .slice()
        .sort(function (a, b) {
          return (b.summary.overall || 0) - (a.summary.overall || 0);
        });

      // doc id -> { model -> doc record }
      var byDoc = {};
      models.forEach(function (model) {
        (model.docs || []).forEach(function (doc) {
          if (!byDoc[doc.id]) byDoc[doc.id] = {};
          byDoc[doc.id][model.model] = doc;
        });
      });

      var rows = (manifest.documents || [])
        .filter(function (doc) {
          if (category && doc.category !== category) return false;
          if (query) {
            var haystack = (doc.id + " " + doc.category + " " + (doc.tags || []).join(" ")).toLowerCase();
            if (haystack.indexOf(query) === -1) return false;
          }
          return byDoc[doc.id] !== undefined;
        })
        .map(function (doc) {
          var scores = {};
          var sum = 0;
          var count = 0;
          models.forEach(function (model) {
            var record = byDoc[doc.id] && byDoc[doc.id][model.model];
            var value = record && record.metrics ? record.metrics.char_similarity * 100 : null;
            if (record && record.error) value = 0;
            scores[model.model] = { value: value, error: record && record.error };
            if (typeof value === "number") {
              sum += value;
              count += 1;
            }
          });
          return { doc: doc, scores: scores, mean: count ? sum / count : null };
        });

      var allValues = [];
      rows.forEach(function (row) {
        models.forEach(function (model) {
          allValues.push(row.scores[model.model].value);
        });
      });
      var hue = heatScale(allValues);

      var columns = [
        {
          key: "id",
          label: "Document",
          value: function (row) {
            return row.doc.id;
          },
          asc: true,
          cell: function (row) {
            return {
              html:
                '<a href="' +
                esc(href("/doc/" + encodeURIComponent(row.doc.id), {})) +
                '">' +
                esc(row.doc.id) +
                "</a>",
              cls: "doc",
            };
          },
        },
        {
          key: "category",
          label: "Category",
          value: function (row) {
            return row.doc.category;
          },
          asc: true,
          cell: function (row) {
            return { html: esc(row.doc.category), cls: "muted" };
          },
        },
        {
          key: "pages",
          label: "Pages",
          num: true,
          value: function (row) {
            return row.doc.pages;
          },
          cell: function (row) {
            return { html: esc(row.doc.pages) };
          },
        },
        {
          key: "mean",
          label: "Mean",
          num: true,
          title: "Mean overall score across every model in this run",
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
            title: "Overall score of " + model.model + " on this document",
            value: function (row) {
              return row.scores[model.model].value;
            },
            cell: function (row) {
              var entry = row.scores[model.model];
              if (entry.error) return { html: "failed", cls: "fail", title: entry.error };
              if (typeof entry.value !== "number") return { html: "—", cls: "muted" };
              return {
                html:
                  '<a href="' +
                  esc(
                    href("/doc/" + encodeURIComponent(row.doc.id), { model: model.model })
                  ) +
                  '">' +
                  esc(fixed(entry.value, 1)) +
                  "</a>",
                cls: "heat",
                style: "--hm:" + hue(entry.value),
              };
            },
          };
        })
      );

      var categories = (runInfo.categories || []).slice();
      var options =
        '<option value="">All categories</option>' +
        categories
          .map(function (name) {
            return (
              '<option value="' +
              esc(name) +
              '"' +
              (name === category ? " selected" : "") +
              ">" +
              esc(name) +
              "</option>"
            );
          })
          .join("");

      var sortKey = params.get("sort") || "id";
      var sortDir = params.get("dir") || (sortKey === "id" || sortKey === "category" ? "asc" : "desc");

      view.innerHTML =
        "<h1>Documents</h1>" +
        '<div class="toolbar">' +
        runSelector(runInfo) +
        '<div class="field"><label for="cat-select">Category</label><select id="cat-select">' +
        options +
        "</select></div>" +
        '<div class="field"><label for="doc-search">Filter</label>' +
        '<input id="doc-search" type="search" placeholder="id or tag" value="' +
        esc(params.get("q") || "") +
        '" /></div>' +
        '<p class="hint">' +
        rows.length +
        " of " +
        (manifest.documents || []).length +
        " documents. Click one to see the input, the ground truth and every model's output.</p>" +
        "</div>" +
        runMeta(runInfo) +
        sortableTable({
          columns: columns,
          rows: rows,
          sortKey: sortKey,
          sortDir: sortDir,
          caption:
            "Per-document overall score (100 × character similarity) for each model. " +
            "Click a column heading to sort, or a score to open that document with that model selected.",
        });

      wireSorting(view, "/documents");
      wireRunSelect(view, "/documents");

      var categorySelect = view.querySelector("#cat-select");
      categorySelect.addEventListener("change", function () {
        go("/documents", { cat: categorySelect.value, q: params.get("q"), sort: params.get("sort"), dir: params.get("dir") });
      });
      var search = view.querySelector("#doc-search");
      var timer = null;
      search.addEventListener("input", function () {
        clearTimeout(timer);
        timer = setTimeout(function () {
          go("/documents", {
            cat: category,
            q: search.value,
            sort: params.get("sort"),
            dir: params.get("dir"),
          });
          var again = document.getElementById("doc-search");
          if (again) {
            again.focus();
            again.setSelectionRange(again.value.length, again.value.length);
          }
        }, 220);
      });
    });
  }

  /* ---------------------------------------------------------- word-level diff */

  /** Split text into {word, pre} tokens plus the trailing whitespace. */
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

  /** Comparison key: mirrors the benchmark's normalisation closely enough to be fair. */
  function tokenKey(word) {
    var key = word.toLowerCase();
    if (key.normalize) key = key.normalize("NFKC");
    key = key
      .replace(/[‘’]/g, "'")
      .replace(/[“”]/g, '"')
      .replace(/[–—]/g, "-");
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
        // Pathologically large middle: report it wholesale rather than freezing the tab.
        for (i = 0; i < an; i++) ops.push({ t: "del", ai: start + i });
        for (i = 0; i < bn; i++) ops.push({ t: "ins", bi: start + i });
      } else {
        var width = bn + 1;
        var lcs = new Int32Array((an + 1) * width);
        var x, y;
        for (x = an - 1; x >= 0; x--) {
          for (y = bn - 1; y >= 0; y--) {
            if (a[start + x] === b[start + y]) {
              lcs[x * width + y] = lcs[(x + 1) * width + y + 1] + 1;
            } else {
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
        while (x < an) {
          ops.push({ t: "del", ai: start + x });
          x++;
        }
        while (y < bn) {
          ops.push({ t: "ins", bi: start + y });
          y++;
        }
      }
    }

    for (i = end - 1; i >= 0; i--) ops.push({ t: "eq", ai: n - 1 - i, bi: m - 1 - i });
    return ops;
  }

  function renderToken(token, tag) {
    var text = esc(token.pre) + (tag ? "<" + tag + ' class="d">' + esc(token.word) + "</" + tag + ">" : esc(token.word));
    return text;
  }

  function renderSide(parsed, marks, markTag) {
    var out = "";
    for (var i = 0; i < parsed.tokens.length; i++) {
      out += renderToken(parsed.tokens[i], marks[i] ? markTag : null);
    }
    return out + esc(parsed.tail);
  }

  function renderUnified(truthParsed, predParsed, ops) {
    var out = "";
    ops.forEach(function (op) {
      if (op.t === "eq") out += renderToken(predParsed.tokens[op.bi], null);
      else if (op.t === "del") out += renderToken(truthParsed.tokens[op.ai], "del");
      else out += renderToken(predParsed.tokens[op.bi], "ins");
    });
    return out;
  }

  /* ---------------------------------------------------------- document view */

  function metricTile(label, value, title) {
    return (
      '<div class="metric"><dt' +
      (title ? ' title="' + esc(title) + '"' : "") +
      ">" +
      esc(label) +
      "</dt><dd>" +
      esc(value) +
      "</dd></div>"
    );
  }

  function viewDoc(docId, params) {
    var runInfo = resolveRun(params);
    if (!runInfo) throw new Error("No benchmark runs found in data/index.json");
    if (!docId) throw new Error("No document id in the URL");

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
        return (b.summary.overall || 0) - (a.summary.overall || 0);
      });
      var selected = params.get("model");
      var model = null;
      models.forEach(function (candidate) {
        if (candidate.model === selected) model = candidate;
      });
      if (!model) model = models[0];

      var docRecord = null;
      (model.docs || []).forEach(function (doc) {
        if (doc.id === docId) docRecord = doc;
      });

      document.title = docId + " · LiteOCR Benchmark";

      var datasetBase = "data/datasets/" + datasetName + "/";
      var truthUrl = datasetBase + entry.truth;
      var inputUrl = datasetBase + entry.file;
      var previewUrl = entry.preview ? datasetBase + entry.preview : null;
      var predUrl =
        "data/outputs/" + run.run_id + "/" + slugOf(model.model) + "/" + docId + ".md";
      var isPdf = /\.pdf$/i.test(entry.file);
      var diffMode = params.get("diff") === "unified" ? "unified" : "split";

      return Promise.all([
        getText(truthUrl),
        getText(predUrl).catch(function () {
          return null;
        }),
      ]).then(function (texts) {
        var truth = texts[0];
        var pred = texts[1];

        var modelOptions = models
          .map(function (candidate) {
            var record = null;
            (candidate.docs || []).forEach(function (doc) {
              if (doc.id === docId) record = doc;
            });
            var score =
              record && record.metrics ? " · " + fixed(record.metrics.char_similarity * 100, 1) : "";
            return (
              '<option value="' +
              esc(candidate.model) +
              '"' +
              (candidate.model === model.model ? " selected" : "") +
              ">" +
              esc(candidate.model + score) +
              "</option>"
            );
          })
          .join("");

        var metrics = (docRecord && docRecord.metrics) || {};
        var tiles =
          metricTile(
            "Overall",
            typeof metrics.char_similarity === "number" ? fixed(metrics.char_similarity * 100, 2) : "—",
            "100 × character similarity for this document"
          ) +
          metricTile("Char sim", fixed(metrics.char_similarity, 4)) +
          metricTile("CER", fixed(metrics.cer, 4)) +
          metricTile("WER", fixed(metrics.wer, 4)) +
          metricTile("Word F1", fixed(metrics.word_f1, 4)) +
          metricTile("Word recall", fixed(metrics.word_recall, 4)) +
          metricTile("Word precision", fixed(metrics.word_precision, 4)) +
          metricTile("Order", fixed(metrics.order_score, 4)) +
          metricTile("Table", fixed(metrics.table_score, 4)) +
          metricTile("Latency", docRecord ? msFmt(docRecord.latency_ms) : "—", "Client-measured, caches disabled") +
          metricTile(
            "Cost",
            docRecord && typeof docRecord.cost_usd === "number" ? "$" + docRecord.cost_usd.toFixed(4) : "—",
            "List price for this document"
          ) +
          metricTile(
            "Chars",
            (metrics.pred_chars != null ? metrics.pred_chars : "—") +
              " / " +
              (metrics.truth_chars != null ? metrics.truth_chars : "—"),
            "Predicted / ground-truth characters after normalisation"
          );

        var diffHtml;
        var diffStats = "";
        if (pred === null) {
          diffHtml =
            '<p class="empty">No saved output for <code>' +
            esc(model.model) +
            "</code> on this document" +
            (docRecord && docRecord.error ? " — the call failed: " + esc(docRecord.error) : "") +
            ".</p>";
        } else {
          var truthParsed = tokenize(truth);
          var predParsed = tokenize(pred);
          var truthKeys = truthParsed.tokens.map(function (token) {
            return tokenKey(token.word);
          });
          var predKeys = predParsed.tokens.map(function (token) {
            return tokenKey(token.word);
          });
          var ops = diffKeys(truthKeys, predKeys);
          var deletions = 0;
          var insertions = 0;
          var equal = 0;
          var truthMarks = new Array(truthParsed.tokens.length);
          var predMarks = new Array(predParsed.tokens.length);
          ops.forEach(function (op) {
            if (op.t === "eq") {
              equal++;
            } else if (op.t === "del") {
              deletions++;
              truthMarks[op.ai] = true;
            } else {
              insertions++;
              predMarks[op.bi] = true;
            }
          });
          diffStats =
            equal +
            " words matched · " +
            deletions +
            " missing from the output · " +
            insertions +
            " added by the model";

          if (diffMode === "unified") {
            diffHtml =
              '<div class="pane" tabindex="0" role="region" aria-label="Word-level diff of the model output against the ground truth">' +
              renderUnified(truthParsed, predParsed, ops) +
              "</div>";
          } else {
            diffHtml =
              '<div class="grid-2">' +
              '<div><div class="pane-head"><h3>Ground truth</h3><span class="hint">' +
              esc(entry.truth) +
              '</span></div><div class="pane" tabindex="0" role="region" aria-label="Ground truth with missing words highlighted">' +
              renderSide(truthParsed, truthMarks, "del") +
              "</div></div>" +
              '<div><div class="pane-head"><h3>' +
              esc(model.model) +
              ' output</h3><span class="hint"><a href="' +
              esc(predUrl) +
              '" rel="noopener">raw .md</a></span></div>' +
              '<div class="pane" tabindex="0" role="region" aria-label="Model output with added words highlighted">' +
              renderSide(predParsed, predMarks, "ins") +
              "</div></div>" +
              "</div>";
          }
        }

        var parseCommand =
          "liteocr parse benchmark/datasets/" +
          datasetName +
          "/" +
          entry.file +
          " -m " +
          model.model +
          " > " +
          docId +
          ".pred.md";
        var scoreCommand =
          "liteocr bench score " +
          docId +
          ".pred.md benchmark/datasets/" +
          datasetName +
          "/" +
          entry.truth;

        var preview = previewUrl
          ? '<img class="doc-preview" src="' +
            esc(previewUrl) +
            '" alt="Rendered page 1 of input document ' +
            esc(docId) +
            '" loading="lazy" />'
          : '<p class="empty">No inline preview for this input. ' +
            '<a href="' +
            esc(inputUrl) +
            '" rel="noopener">Open the file</a>.</p>';

        view.innerHTML =
          '<a class="back-link" href="' +
          esc(href("/documents", {})) +
          '">← All documents</a>' +
          "<h1>" +
          esc(docId) +
          "</h1>" +
          '<ul class="badges">' +
          '<li class="badge strong">' +
          esc(entry.category) +
          "</li>" +
          '<li class="badge">' +
          esc(entry.pages) +
          (entry.pages === 1 ? " page" : " pages") +
          "</li>" +
          (entry.tags || [])
            .map(function (tag) {
              return '<li class="badge">' + esc(tag) + "</li>";
            })
            .join("") +
          '<li class="badge">' +
          esc(datasetName) +
          " v" +
          esc(manifest.version) +
          "</li>" +
          '<li class="badge">run ' +
          esc(run.run_id) +
          "</li>" +
          "</ul>" +
          '<div class="grid-2">' +
          '<section class="panel"><div class="panel-title"><h2>Input</h2>' +
          '<span class="hint"><a href="' +
          esc(inputUrl) +
          '" rel="noopener">' +
          esc(entry.file) +
          (isPdf ? " (PDF)" : "") +
          "</a></span></div>" +
          preview +
          (isPdf
            ? '<p class="hint" style="margin-top:8px">Preview shows page 1; the benchmark scores all ' +
              esc(entry.pages) +
              " pages. " +
              '<a href="' +
              esc(inputUrl) +
              '" rel="noopener">Download the PDF</a>.</p>'
            : "") +
          "</section>" +
          '<section class="panel"><div class="panel-title"><h2>Ground truth</h2>' +
          '<span class="hint"><a href="' +
          esc(truthUrl) +
          '" rel="noopener">' +
          esc(entry.truth) +
          "</a></span></div>" +
          '<div class="pane" tabindex="0" role="region" aria-label="Ground truth markdown source">' +
          esc(truth) +
          "</div>" +
          '<p class="hint" style="margin-top:8px">Shown as markdown <em>source</em>, not rendered HTML — ' +
          "this is the exact text the metrics compare against.</p>" +
          "</section>" +
          "</div>" +
          '<section class="panel">' +
          '<div class="toolbar" style="margin-bottom:12px">' +
          '<div class="field"><label for="model-select">Model</label><select id="model-select">' +
          modelOptions +
          "</select></div>" +
          '<div class="field"><label id="diff-label">Diff view</label>' +
          '<div class="seg" role="group" aria-labelledby="diff-label">' +
          '<button type="button" data-diff="split" aria-pressed="' +
          (diffMode === "split") +
          '">Side by side</button>' +
          '<button type="button" data-diff="unified" aria-pressed="' +
          (diffMode === "unified") +
          '">Unified</button>' +
          "</div></div>" +
          '<p class="hint">' +
          esc(diffStats) +
          "</p>" +
          "</div>" +
          '<dl class="metrics-grid">' +
          tiles +
          "</dl>" +
          '<div class="diff-legend" style="margin-top:14px">' +
          '<span><span class="swatch del"></span>in the ground truth, missing from the output</span>' +
          '<span><span class="swatch ins"></span>in the output, not in the ground truth</span>' +
          "<span>word-level LCS diff, compared after the same case/markdown normalisation the scorer uses</span>" +
          "</div>" +
          diffHtml +
          "</section>" +
          '<section class="panel"><h2>Verify it yourself</h2>' +
          "<p>Run the same document through the same model and score it locally — no benchmark harness " +
          "involved, just the CLI:</p>" +
          codeBlock(parseCommand) +
          codeBlock(scoreCommand) +
          '<p class="hint">' +
          "The truth file and this model's saved output are served straight from this site: " +
          '<a href="' +
          esc(truthUrl) +
          '" rel="noopener">ground truth</a>' +
          (pred !== null ? ' · <a href="' + esc(predUrl) + '" rel="noopener">model output</a>' : "") +
          ' · <a href="' +
          esc(inputUrl) +
          '" rel="noopener">input document</a>. ' +
          "Latency and cost above are from the recorded run; re-running will give you fresh latency for " +
          "your own region and network.</p>" +
          "</section>";

        wireCopyButtons(view);
        var modelSelect = view.querySelector("#model-select");
        modelSelect.addEventListener("change", function () {
          go("/doc/" + encodeURIComponent(docId), { model: modelSelect.value, diff: diffMode });
        });
        view.querySelectorAll("button[data-diff]").forEach(function (button) {
          button.addEventListener("click", function () {
            go("/doc/" + encodeURIComponent(docId), {
              model: model.model,
              diff: button.getAttribute("data-diff"),
            });
          });
        });
      });
    });
  }

  /* ------------------------------------------------------------------ boot */

  function render() {
    var route = parseHash();
    var name = route.parts[0] || "leaderboard";
    setNav(name === "doc" ? "documents" : name);
    view.setAttribute("aria-busy", "true");

    var path = route.parts.join("/");
    var task;
    try {
      if (name === "documents") task = Promise.resolve(viewDocuments(route.params));
      else if (name === "doc") task = Promise.resolve(viewDoc(route.parts[1], route.params));
      else task = Promise.resolve(viewLeaderboard(route.params));
    } catch (err) {
      showError(err);
      return;
    }
    task.then(
      function () {
        view.removeAttribute("aria-busy");
        if (path !== lastPath) {
          window.scrollTo(0, 0);
          lastPath = path;
        }
      },
      function (err) {
        showError(err);
      }
    );
  }

  function boot() {
    getJSON("data/index.json").then(
      function (data) {
        INDEX = data;
        var first = (data.runs || [])[0];
        var dataset = first ? (data.datasets || {})[first.dataset.name] : null;
        if (dataset) {
          var nameEl = document.getElementById("footer-dataset");
          var licenseEl = document.getElementById("footer-license");
          if (nameEl) nameEl.textContent = dataset.name + " v" + dataset.version;
          if (licenseEl) licenseEl.textContent = dataset.license || "see the repository";
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
