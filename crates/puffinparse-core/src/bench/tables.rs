//! Table extraction for scoring: markdown pipe tables **and** HTML `<table>`s, both reduced to the
//! same row/cell grid, plus a TEDS-style structure metric over that grid.
//!
//! Providers disagree on how they emit tables: most write GitHub pipe tables, Reducto (`r-1`,
//! sometimes `standard`) and LlamaParse write HTML when a table has merged cells. A scorer that only
//! reads one of the two scores a correct table as 0, so both are parsed here into a [`Grid`].
//!
//! The HTML reader is deliberately small and tolerant (no html5ever): it understands `table`,
//! `tr`, `td`/`th` with `colspan`/`rowspan`, ignores `thead`/`tbody`/`tfoot` boundaries, drops
//! inline markup but keeps its text, turns `br`/`p`/`li`/`div` into a space, decodes character
//! references, and closes unclosed cells and rows implicitly. Spanning cells are **repeated** into
//! every grid slot they cover, which is exactly how the ParseBench adapter
//! (`benchmark/adapters/base.py::html_table_to_markdown`) wrote the ground truth, so prediction and
//! truth go through the same transformation.

/// One table as rows of raw (un-normalised) cell text. The first row is the header row.
pub type Grid = Vec<Vec<String>>;

/// Largest `colspan`/`rowspan` honoured; anything else (0, negative, huge, garbage) counts as 1.
/// Matches the ParseBench adapter's `_span`.
const MAX_SPAN: usize = 64;

/// Every table of `doc`, markdown and HTML, in document order.
pub fn extract_tables(doc: &str) -> Vec<Grid> {
    let lower = doc.to_ascii_lowercase(); // ASCII lowercasing keeps byte offsets identical
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = find_table_open(&lower, pos) {
        out.extend(markdown_tables(&doc[pos..start]));
        let (grid, consumed) = parse_html_table(&doc[start..]);
        if !grid.is_empty() {
            out.push(grid);
        }
        pos = start + consumed.max(1);
        // `consumed.max(1)` guarantees progress; keep `pos` on a char boundary.
        while pos < doc.len() && !doc.is_char_boundary(pos) {
            pos += 1;
        }
    }
    out.extend(markdown_tables(&doc[pos.min(doc.len())..]));
    out
}

/// Byte offset of the next `<table` tag (not `<tablefoo`) at or after `from` in lowercased text.
fn find_table_open(lower: &str, from: usize) -> Option<usize> {
    let mut at = from;
    while let Some(i) = lower.get(at..)?.find("<table") {
        let start = at + i;
        match lower.as_bytes().get(start + 6) {
            None => return Some(start),
            Some(b) if b.is_ascii_whitespace() || *b == b'>' || *b == b'/' => return Some(start),
            _ => at = start + 6,
        }
    }
    None
}

// ---- markdown ------------------------------------------------------------------------------------

/// Markdown pipe tables: runs of consecutive lines starting with `|`, separator rows dropped.
pub fn markdown_tables(md: &str) -> Vec<Grid> {
    let mut tables = Vec::new();
    let mut current: Grid = Vec::new();
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            if !current.is_empty() {
                tables.push(std::mem::take(&mut current));
            }
            continue;
        }
        if is_separator_row(t) {
            continue;
        }
        current.push(split_row(t));
    }
    if !current.is_empty() {
        tables.push(current);
    }
    tables
}

/// `|---|:--:|` and friends.
pub fn is_separator_row(line: &str) -> bool {
    line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Split one `| a | b |` row into its cells, honouring `\|` escapes.
pub fn split_row(line: &str) -> Vec<String> {
    let mut cells = vec![String::new()];
    let mut escaped = false;
    for c in line.trim().trim_start_matches('|').chars() {
        let cur = cells.last_mut().expect("non-empty");
        if escaped {
            if c != '|' {
                cur.push('\\');
            }
            cur.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '|' {
            cells.push(String::new());
        } else {
            cur.push(c);
        }
    }
    if escaped {
        cells.last_mut().expect("non-empty").push('\\');
    }
    // A trailing `|` leaves an empty cell that is formatting, not data.
    if cells.last().is_some_and(|c| c.trim().is_empty()) && cells.len() > 1 {
        cells.pop();
    }
    cells.iter().map(|c| c.trim().to_string()).collect()
}

// ---- HTML ----------------------------------------------------------------------------------------

#[derive(Debug)]
struct Cell {
    text: String,
    colspan: usize,
    rowspan: usize,
}

/// Parse the HTML table starting at `html[0]` (`<table…`). Returns the expanded grid and the number
/// of bytes consumed (up to and including the matching `</table>`, or all of `html` if unclosed).
/// Nested tables are flattened into the text of the enclosing cell.
pub fn parse_html_table(html: &str) -> (Grid, usize) {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Option<Vec<Cell>> = None;
    let mut cell: Option<Cell> = None;
    let mut depth = 0usize; // nesting depth *inside* the outer table
    let mut started = false;
    let bytes = html.as_bytes();
    let mut i = 0;
    let mut text_start = 0;

    fn close_cell(cell: &mut Option<Cell>, row: &mut Option<Vec<Cell>>) {
        if let Some(c) = cell.take() {
            row.get_or_insert_with(Vec::new).push(c);
        }
    }
    fn close_row(cell: &mut Option<Cell>, row: &mut Option<Vec<Cell>>, rows: &mut Vec<Vec<Cell>>) {
        close_cell(cell, row);
        if let Some(r) = row.take() {
            if !r.is_empty() {
                rows.push(r);
            }
        }
    }

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Comments.
        if html[i..].starts_with("<!--") {
            if let Some(c) = cell.as_mut() {
                c.text.push_str(&decode_entities(&html[text_start..i]));
            }
            let end = html[i + 4..].find("-->").map(|e| i + 4 + e + 3).unwrap_or(bytes.len());
            i = end;
            text_start = i;
            continue;
        }
        // An unterminated `<` or one that does not open a tag (`a < b`) is text.
        let Some(rel_end) = html[i..].find('>') else { break };
        let tag_end = i + rel_end + 1;
        let inner = &html[i + 1..tag_end - 1];
        let (closing, rest) = match inner.strip_prefix('/') {
            Some(r) => (true, r),
            None => (false, inner),
        };
        let name_len = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
        let name = rest[..name_len].to_ascii_lowercase();
        let attrs = &rest[name_len..];
        if name.is_empty() || !rest.as_bytes()[0].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        // Flush the text before the tag into the open cell.
        if let Some(c) = cell.as_mut() {
            c.text.push_str(&decode_entities(&html[text_start..i]));
        }
        i = tag_end;
        text_start = i;

        match (closing, name.as_str()) {
            (false, "table") => {
                if started {
                    depth += 1;
                    if let Some(c) = cell.as_mut() {
                        c.text.push(' ');
                    }
                } else {
                    started = true;
                }
            }
            (true, "table") => {
                if depth > 0 {
                    depth -= 1;
                    if let Some(c) = cell.as_mut() {
                        c.text.push(' ');
                    }
                } else {
                    close_row(&mut cell, &mut row, &mut rows);
                    return (expand(rows), tag_end);
                }
            }
            _ if depth > 0 => {
                // Inside a nested table everything is text of the outer cell.
                if let Some(c) = cell.as_mut() {
                    c.text.push(' ');
                }
            }
            (false, "tr") => close_row(&mut cell, &mut row, &mut rows),
            (true, "tr") => close_row(&mut cell, &mut row, &mut rows),
            (false, "td" | "th") => {
                close_cell(&mut cell, &mut row);
                cell = Some(Cell {
                    text: String::new(),
                    colspan: span_attr(attrs, "colspan"),
                    rowspan: span_attr(attrs, "rowspan"),
                });
            }
            (true, "td" | "th") => close_cell(&mut cell, &mut row),
            (_, "br" | "p" | "li" | "div") => {
                if let Some(c) = cell.as_mut() {
                    c.text.push(' ');
                }
            }
            // `thead`, `tbody`, `b`, `i`, `span`, `sup`, …: keep the text, drop the tag.
            _ => {}
        }
    }
    if let Some(c) = cell.as_mut() {
        c.text.push_str(&decode_entities(&html[text_start.min(html.len())..]));
    }
    close_row(&mut cell, &mut row, &mut rows);
    (expand(rows), html.len())
}

/// Read `colspan="3"` / `rowspan=2` from a tag's attribute string; invalid values mean 1.
fn span_attr(attrs: &str, key: &str) -> usize {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(key) {
        let at = from + i;
        from = at + key.len();
        // Must be a whole attribute name.
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = lower[at + key.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=').filter(|_| before_ok) else { continue };
        let rest = rest.trim_start().trim_start_matches(['"', '\'']);
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        return match digits.parse::<usize>() {
            Ok(n) if (1..=MAX_SPAN).contains(&n) => n,
            _ => 1,
        };
    }
    1
}

/// Place cells on the grid, repeating spanning cells into every slot they cover, and pad rows to
/// the widest one (empty strings), as the ParseBench truth does.
fn expand(rows: Vec<Vec<Cell>>) -> Grid {
    let mut grid: Vec<Vec<Option<String>>> = Vec::new();
    for (r, row) in rows.into_iter().enumerate() {
        if grid.len() <= r {
            grid.resize_with(r + 1, Vec::new);
        }
        let mut c = 0;
        for cell in row {
            while grid[r].get(c).is_some_and(Option::is_some) {
                c += 1;
            }
            let text = collapse_ws(&cell.text);
            for dr in 0..cell.rowspan {
                let rr = r + dr;
                if grid.len() <= rr {
                    grid.resize_with(rr + 1, Vec::new);
                }
                for dc in 0..cell.colspan {
                    let cc = c + dc;
                    if grid[rr].len() <= cc {
                        grid[rr].resize(cc + 1, None);
                    }
                    grid[rr][cc] = Some(text.clone());
                }
            }
            c += cell.colspan;
        }
    }
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    grid.into_iter()
        .map(|row| {
            let mut row: Vec<String> = row.into_iter().map(Option::unwrap_or_default).collect();
            row.resize(width, String::new());
            row
        })
        // An all-empty row reads as a separator in markdown (`|  |  |`), so drop it here too.
        .filter(|row| row.iter().any(|c| !c.is_empty()))
        .collect()
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode HTML character references: the five XML entities, `&nbsp;` and a few common typographic
/// ones, and numeric `&#NNN;` / `&#xHH;`. Unknown entities are left as written.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let semi = tail.as_bytes()[..tail.len().min(12)].iter().position(|&b| b == b';');
        let decoded = semi.and_then(|j| {
            let name = &tail[1..j];
            let ch = if let Some(num) = name.strip_prefix('#') {
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse::<u32>().ok(),
                };
                code.and_then(char::from_u32)
            } else {
                match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    "ndash" => Some('\u{2013}'),
                    "mdash" => Some('\u{2014}'),
                    "lsquo" => Some('\u{2018}'),
                    "rsquo" => Some('\u{2019}'),
                    "ldquo" => Some('\u{201C}'),
                    "rdquo" => Some('\u{201D}'),
                    "hellip" => Some('\u{2026}'),
                    "deg" => Some('\u{b0}'),
                    "plusmn" => Some('\u{b1}'),
                    "times" => Some('\u{d7}'),
                    "copy" => Some('\u{a9}'),
                    "reg" => Some('\u{ae}'),
                    "euro" => Some('\u{20ac}'),
                    "pound" => Some('\u{a3}'),
                    "cent" => Some('\u{a2}'),
                    "sect" => Some('\u{a7}'),
                    "para" => Some('\u{b6}'),
                    "middot" => Some('\u{b7}'),
                    "bull" => Some('\u{2022}'),
                    _ => None,
                }
            };
            ch.map(|c| (c, j + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- TEDS on the grid ----------------------------------------------------------------------------
//
// TEDS (Zhong et al., 2020, "Image-based table recognition: data, model, and evaluation") scores a
// predicted table against the truth as `1 - TED(Tp, Tt) / max(|Tp|, |Tt|)`, where TED is the tree
// edit distance between the two HTML trees (`table > thead/tbody > tr > td`), insert and delete cost
// 1, and renaming a `td` into another costs the normalised Levenshtein distance of the contents
// (or 1 if their `colspan`/`rowspan` differ).
//
// Our ground truth is markdown (spans already repeated into their slots), so we cannot compare span
// attributes. `teds_grid` is therefore TEDS on the *grid* tree `table > tr > td` of the expanded
// grids — the same formula, costs and Zhang–Shasha edit distance, just without `thead`/`tbody` and
// span attributes. It still punishes missing, extra, split or merged rows and cells, which the
// flat `table_score` string similarity does not see.
//
// Credit: the metric is from the paper above; its reference implementation is IBM's PubTabNet
// `src/metric.py` (github.com/ibm-aur-nlp/PubTabNet, Apache-2.0), which uses APTED. This file is an
// independent Rust implementation (Zhang & Shasha, 1989, "Simple fast algorithms for the editing
// distance between trees and related problems"); no PubTabNet code is copied.

/// Node budget above which [`teds_grid`] declines (`None`): Zhang–Shasha is O(n·m) memory.
const TEDS_MAX_PAIRS: usize = 16_000_000;

/// A node of the grid tree in post-order.
#[derive(Debug, Clone)]
enum Label {
    Table,
    Row,
    Cell(Vec<char>),
}

struct Tree {
    labels: Vec<Label>,
    /// Post-order index of the leftmost leaf descendant of each node.
    lml: Vec<usize>,
    keyroots: Vec<usize>,
}

impl Tree {
    /// `cells` are already normalised.
    fn from_grid(grid: &[Vec<String>]) -> Self {
        let mut labels = Vec::new();
        let mut lml = Vec::new();
        for row in grid {
            let row_first = labels.len();
            for cell in row {
                lml.push(labels.len());
                labels.push(Label::Cell(cell.chars().collect()));
            }
            // A row's leftmost leaf is its first cell, or itself when empty.
            lml.push(row_first);
            labels.push(Label::Row);
        }
        lml.push(0);
        labels.push(Label::Table);
        // Keyroots: for each distinct lml, the highest node (largest post-order index) with it.
        let mut seen = std::collections::HashMap::new();
        for (i, &l) in lml.iter().enumerate() {
            seen.insert(l, i);
        }
        let mut keyroots: Vec<usize> = seen.into_values().collect();
        keyroots.sort_unstable();
        Self { labels, lml, keyroots }
    }

    fn len(&self) -> usize {
        self.labels.len()
    }
}

fn rename_cost(a: &Label, b: &Label) -> f64 {
    match (a, b) {
        (Label::Table, Label::Table) | (Label::Row, Label::Row) => 0.0,
        (Label::Cell(x), Label::Cell(y)) => {
            let m = x.len().max(y.len());
            if m == 0 {
                0.0
            } else {
                super::levenshtein(x, y) as f64 / m as f64
            }
        }
        _ => 1.0,
    }
}

/// Zhang–Shasha tree edit distance with unit insert/delete and [`rename_cost`].
fn tree_edit_distance(a: &Tree, b: &Tree) -> f64 {
    let (n, m) = (a.len(), b.len());
    let mut td = vec![0.0f64; n * m];
    let mut fd = vec![0.0f64; (n + 1) * (m + 1)];
    for &i in &a.keyroots {
        for &j in &b.keyroots {
            let (li, lj) = (a.lml[i], b.lml[j]);
            let (rows, cols) = (i - li + 2, j - lj + 2);
            // fd is indexed [x][y] with x in 0..rows, y in 0..cols; offsets li-1 / lj-1.
            let at = |x: usize, y: usize| x * cols + y;
            fd[at(0, 0)] = 0.0;
            for x in 1..rows {
                fd[at(x, 0)] = fd[at(x - 1, 0)] + 1.0;
            }
            for y in 1..cols {
                fd[at(0, y)] = fd[at(0, y - 1)] + 1.0;
            }
            for x in 1..rows {
                let i1 = li + x - 1;
                for y in 1..cols {
                    let j1 = lj + y - 1;
                    let del = fd[at(x - 1, y)] + 1.0;
                    let ins = fd[at(x, y - 1)] + 1.0;
                    let v = if a.lml[i1] == li && b.lml[j1] == lj {
                        let v = del.min(ins).min(fd[at(x - 1, y - 1)] + rename_cost(&a.labels[i1], &b.labels[j1]));
                        td[i1 * m + j1] = v;
                        v
                    } else {
                        let px = a.lml[i1] - li; // forest before i1's subtree
                        let py = b.lml[j1] - lj;
                        del.min(ins).min(fd[at(px, py)] + td[i1 * m + j1])
                    };
                    fd[at(x, y)] = v;
                }
            }
        }
    }
    td[(n - 1) * m + (m - 1)]
}

/// TEDS between two already-normalised grids, in `0..=1`. `None` when the pair is too large.
pub fn teds_pair(pred: &[Vec<String>], truth: &[Vec<String>]) -> Option<f64> {
    let (a, b) = (Tree::from_grid(pred), Tree::from_grid(truth));
    if a.len().saturating_mul(b.len()) > TEDS_MAX_PAIRS {
        return None;
    }
    let dist = tree_edit_distance(&a, &b);
    Some((1.0 - dist / a.len().max(b.len()) as f64).clamp(0.0, 1.0))
}

/// Document-level `teds_grid`: for every truth table, the best TEDS against any predicted table;
/// then the mean over truth tables. Extra predicted tables are not penalised (a `table-only` truth is
/// only the page's table while the prediction is the whole page). `None` when the truth has no
/// table; `Some(0.0)` when the prediction has none.
pub fn teds_grid(pred: &[Grid], truth: &[Grid]) -> Option<f64> {
    if truth.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for t in truth {
        let best = pred.iter().filter_map(|p| teds_pair(p, t)).fold(0.0f64, f64::max);
        total += best;
    }
    Some(total / truth.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(rows: &[&[&str]]) -> Grid {
        rows.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn markdown_tables_split_and_skip_separators() {
        let md = "intro\n| a | b |\n|---|:-:|\n| 1 | 2 |\ntext\n| x |\n";
        assert_eq!(extract_tables(md), vec![g(&[&["a", "b"], &["1", "2"]]), g(&[&["x"]])]);
    }

    #[test]
    fn html_table_basic_with_thead_tbody_th_td() {
        let html = "<table><thead><tr><th>Name</th><th>Qty</th></tr></thead>\
                    <tbody><tr><td><b>Apple</b></td><td> 3 </td></tr><tr><td>Pear &amp; Fig</td><td>4</td></tr></tbody></table>";
        let t = extract_tables(html);
        assert_eq!(t, vec![g(&[&["Name", "Qty"], &["Apple", "3"], &["Pear & Fig", "4"]])]);
    }

    #[test]
    fn html_spans_repeat_into_every_slot() {
        let html = r#"<table><tr><th colspan="2">Head</th><th>C</th></tr>
            <tr><td rowspan=2>r</td><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>"#;
        assert_eq!(extract_tables(html), vec![g(&[&["Head", "Head", "C"], &["r", "a", "b"], &["r", "c", "d"]])]);
    }

    #[test]
    fn html_is_tolerant_of_missing_close_tags_and_odd_attrs() {
        let html = "<TABLE border=1><tr><td>1<td>2<tr><td colspan='x'>3<td colspan=\"999\">4</table>tail";
        assert_eq!(extract_tables(html), vec![g(&[&["1", "2"], &["3", "4"]])]);
        // Unclosed table: runs to the end of input.
        let open = "<table><tr><td>a</td><td>b<br>c</td>";
        assert_eq!(extract_tables(open), vec![g(&[&["a", "b c"]])]);
        // `<tablex>` is not a table.
        assert!(extract_tables("<tablex>no</tablex>").is_empty());
    }

    #[test]
    fn nested_tables_flatten_into_the_outer_cell() {
        let html = "<table><tr><td>out<table><tr><td>in</td></tr></table></td><td>z</td></tr></table>";
        assert_eq!(extract_tables(html), vec![g(&[&["out in", "z"]])]);
    }

    #[test]
    fn mixed_markdown_and_html_keep_document_order() {
        let doc = "| m |\n|---|\n| 1 |\n\n<table><tr><td>h</td></tr></table>\n\n| n |\n|---|\n| 2 |";
        let t = extract_tables(doc);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0][0][0], "m");
        assert_eq!(t[1][0][0], "h");
        assert_eq!(t[2][0][0], "n");
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#233; &#x41; &nbsp;&bogus; &"),
            "a & b <c> é A \u{a0}&bogus; &"
        );
    }

    /// A real Reducto `r-1` table (ParseBench `637951191e7b…_page1`), trimmed to its header and
    /// first data group, against the adapter's markdown rendering of the same structure.
    #[test]
    fn real_reducto_r1_html_table() {
        let html = r#"<table>
<thead>
<tr>
<th colspan="12"> <b><i>Table 2. Control of volunteer RR 2Xtend alfalfa</i></b> </th>
</tr>
<tr>
<th colspan="3"> Pest Name </th>
<th colspan="9"> <b>VOLUNTEER ALFALFA</b> </th>
</tr>
</thead>
<tbody>
<tr>
<td rowspan="2"> 1 </td>
<td> SONIC </td>
<td> 6.4 </td>
<td> oz wt/a </td>
<td rowspan="2"> <b>24</b> </td>
</tr>
<tr>
<td> FLEXSTAR GT 3.5 </td>
<td> 3 </td>
<td> pt/a </td>
</tr>
</tbody>
</table>"#;
        let t = extract_tables(html);
        assert_eq!(t.len(), 1);
        let grid = &t[0];
        assert_eq!(grid.len(), 4);
        assert!(grid.iter().all(|r| r.len() == 12), "padded to the widest row: {grid:?}");
        assert_eq!(grid[0][11], "Table 2. Control of volunteer RR 2Xtend alfalfa");
        assert_eq!(grid[1][..4], ["Pest Name", "Pest Name", "Pest Name", "VOLUNTEER ALFALFA"]);
        assert_eq!(grid[2][..5], ["1", "SONIC", "6.4", "oz wt/a", "24"]);
        // The rowspan cells carry down; the second row's own cells fill the gaps.
        assert_eq!(grid[3][..5], ["1", "FLEXSTAR GT 3.5", "3", "pt/a", "24"]);
    }

    #[test]
    fn teds_known_cases() {
        let t = g(&[&["a", "b"], &["1", "2"]]);
        // Identical.
        assert_eq!(teds_pair(&t, &t), Some(1.0));
        // One cell's content fully wrong: rename cost 1 over 7 nodes.
        let wrong = g(&[&["a", "b"], &["1", "x"]]);
        assert!((teds_pair(&wrong, &t).unwrap() - (1.0 - 1.0 / 7.0)).abs() < 1e-12);
        // A missing row (row node + 2 cells deleted): 3 / 7.
        let short = g(&[&["a", "b"]]);
        assert!((teds_pair(&short, &t).unwrap() - (1.0 - 3.0 / 7.0)).abs() < 1e-12);
        // A missing column: 2 cells deleted of 7.
        let narrow = g(&[&["a"], &["1"]]);
        assert!((teds_pair(&narrow, &t).unwrap() - (1.0 - 2.0 / 7.0)).abs() < 1e-12);
        // Partial content: "ab" vs "ac" costs 0.5.
        let near = g(&[&["a", "b"], &["1", "2x"]]);
        assert!((teds_pair(&near, &t).unwrap() - (1.0 - 0.5 / 7.0)).abs() < 1e-12);
        // Two rows merged into one (same text, different structure) is not free.
        let merged = g(&[&["a 1", "b 2"]]);
        assert!(teds_pair(&merged, &t).unwrap() < 0.75);
        // Symmetric.
        assert_eq!(teds_pair(&t, &short), teds_pair(&short, &t));
    }

    #[test]
    fn teds_document_level_takes_best_match_per_truth_table() {
        let t = g(&[&["a", "b"], &["1", "2"]]);
        let other = g(&[&["zz"]]);
        assert_eq!(teds_grid(&[other.clone(), t.clone()], std::slice::from_ref(&t)), Some(1.0));
        assert_eq!(teds_grid(&[], std::slice::from_ref(&t)), Some(0.0));
        assert_eq!(teds_grid(&[t], &[]), None);
    }
}
