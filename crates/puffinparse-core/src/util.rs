//! Small helpers shared by providers.

use serde_json::Value;

/// Recursively merge `patch` into `base` (objects merge key-wise; anything else is replaced).
pub fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => deep_merge(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// Parse a 1-based page selection like `"1-3,7,10-"` into inclusive ranges.
/// Open-ended ranges (`"10-"`) get `end = None`.
pub fn parse_page_ranges(spec: &str) -> crate::error::Result<Vec<(u32, Option<u32>)>> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (start, end) = match part.split_once('-') {
            Some((s, e)) => {
                let start = s.trim().parse::<u32>().map_err(|_| bad(spec))?;
                let end =
                    if e.trim().is_empty() { None } else { Some(e.trim().parse::<u32>().map_err(|_| bad(spec))?) };
                (start, end)
            }
            None => {
                let n = part.parse::<u32>().map_err(|_| bad(spec))?;
                (n, Some(n))
            }
        };
        if start == 0 || matches!(end, Some(e) if e < start) {
            return Err(bad(spec));
        }
        out.push((start, end));
    }
    if out.is_empty() {
        return Err(bad(spec));
    }
    Ok(out)
}

fn bad(spec: &str) -> crate::error::Error {
    crate::error::Error::input(format!("invalid page selection '{spec}' (expected e.g. \"1-3,7\")"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_recursively() {
        let mut b = json!({"a": {"x": 1, "y": 2}, "b": 1});
        deep_merge(&mut b, &json!({"a": {"y": 3, "z": 4}, "c": [1]}));
        assert_eq!(b, json!({"a": {"x": 1, "y": 3, "z": 4}, "b": 1, "c": [1]}));
    }

    #[test]
    fn parses_pages() {
        assert_eq!(parse_page_ranges("1-3,7,10-").unwrap(), vec![(1, Some(3)), (7, Some(7)), (10, None)]);
        assert!(parse_page_ranges("0").is_err());
        assert!(parse_page_ranges("3-1").is_err());
        assert!(parse_page_ranges("a").is_err());
        assert!(parse_page_ranges("").is_err());
    }
}
