//! JSON, pretty-printed in the terminal's palette. Key order is kept. Small
//! objects and arrays of plain values stay on one line when they fit, and long
//! arrays of numbers fill lines instead of taking one line per number.

use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;

use super::{Line, Span, Wrap, line_width};

const KEY: Style = Style::new().fg(Color::Blue);
const STRING: Style = Style::new().fg(Color::Green);
const NUMBER: Style = Style::new().fg(Color::Yellow);
const BOOL: Style = Style::new().fg(Color::Magenta);
const NULL: Style = Style::new().add_modifier(Modifier::DIM.union(Modifier::ITALIC));
const PUNCT: Style = Style::new().add_modifier(Modifier::DIM);
const PLAIN: Style = Style::new();

/// Objects with more entries than this always open up, even when they fit.
const INLINE_ENTRIES: usize = 4;

/// None when `src` is not valid JSON; the caller shows it as plain code.
pub fn render(src: &str, width: usize) -> Option<Vec<Line>> {
    let value: Value = serde_json::from_str(src).ok()?;
    let mut lines = Vec::new();
    node(&mut lines, width, &value, 0, Vec::new(), false);
    Some(lines)
}

/// Writes `v` starting with `lead` (a key, or nothing) at `depth`.
fn node(out: &mut Vec<Line>, width: usize, v: &Value, depth: usize, lead: Line, comma: bool) {
    let indent = Span::new("  ".repeat(depth), PLAIN);
    let mut first = vec![indent.clone()];
    first.extend(lead);
    let used = line_width(&first) + usize::from(comma);

    if let Some(compact) = compact(v)
        && used + line_width(&compact) <= width
    {
        first.extend(compact);
        push_comma(&mut first, comma);
        out.push(first);
        return;
    }

    let (open, close, entries): (&str, &str, Vec<(Option<&String>, &Value)>) = match v {
        Value::Object(map) if !map.is_empty() => {
            ("{", "}", map.iter().map(|(k, v)| (Some(k), v)).collect())
        }
        Value::Array(items) if !items.is_empty() => {
            ("[", "]", items.iter().map(|v| (None, v)).collect())
        }
        // A scalar too long for the line: wrap it under itself.
        scalar => {
            let rest = vec![Span::new("  ".repeat(depth + 1), PLAIN)];
            let mut wrap = Wrap::new(width, first, rest);
            for span in compact(scalar).unwrap_or_default() {
                wrap.word(&span.text, span.style, None);
            }
            let mut lines = wrap.finish();
            if let Some(last) = lines.last_mut() {
                push_comma(last, comma);
            }
            out.extend(lines);
            return;
        }
    };

    first.push(Span::new(open, PUNCT));
    let fill = entries
        .iter()
        .all(|(_, v)| matches!(v, Value::Number(_) | Value::Bool(_) | Value::Null));
    if fill {
        // Numbers pack into lines like words in a paragraph.
        out.push(first);
        let pad = vec![Span::new("  ".repeat(depth + 1), PLAIN)];
        let mut wrap = Wrap::new(width, pad.clone(), pad);
        let count = entries.len();
        for (i, (_, v)) in entries.into_iter().enumerate() {
            let mut item = compact(v).unwrap_or_default();
            push_comma(&mut item, i + 1 < count);
            wrap.group(&item);
            wrap.text(" ", PLAIN, None);
        }
        out.extend(wrap.finish());
    } else {
        out.push(first);
        let count = entries.len();
        for (i, (key, v)) in entries.into_iter().enumerate() {
            let lead = key
                .map(|k| vec![Span::new(quote(k), KEY), Span::new(": ", PUNCT)])
                .unwrap_or_default();
            node(out, width, v, depth + 1, lead, i + 1 < count);
        }
    }
    let mut last = vec![indent, Span::new(close, PUNCT)];
    push_comma(&mut last, comma);
    out.push(last);
}

/// The one-line form of `v`, or None if it should always open up.
fn compact(v: &Value) -> Option<Line> {
    let scalar = |v: &Value| -> Option<Span> {
        Some(match v {
            Value::Null => Span::new("null", NULL),
            Value::Bool(b) => Span::new(b.to_string(), BOOL),
            Value::Number(n) => Span::new(n.to_string(), NUMBER),
            Value::String(s) => Span::new(quote(s), STRING),
            _ => return None,
        })
    };
    if let Some(span) = scalar(v) {
        return Some(vec![span]);
    }
    let mut line = Vec::new();
    match v {
        Value::Array(items) if items.is_empty() => line.push(Span::new("[]", PUNCT)),
        Value::Object(map) if map.is_empty() => line.push(Span::new("{}", PUNCT)),
        Value::Array(items) => {
            line.push(Span::new("[", PUNCT));
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    line.push(Span::new(", ", PUNCT));
                }
                line.push(scalar(item)?);
            }
            line.push(Span::new("]", PUNCT));
        }
        Value::Object(map) if map.len() <= INLINE_ENTRIES => {
            line.push(Span::new("{ ", PUNCT));
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    line.push(Span::new(", ", PUNCT));
                }
                line.push(Span::new(quote(k), KEY));
                line.push(Span::new(": ", PUNCT));
                line.push(scalar(item)?);
            }
            line.push(Span::new(" }", PUNCT));
        }
        _ => return None,
    }
    Some(line)
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn push_comma(line: &mut Line, comma: bool) {
    if comma {
        line.push(Span::new(",", PUNCT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(src: &str, width: usize) -> Vec<String> {
        render(src, width)
            .unwrap()
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect()
    }

    #[test]
    fn keeps_small_things_on_one_line() {
        let src =
            r#"{"name":"lsd","tags":["cli","rust"],"pos":{"x":1,"y":2},"deps":{"a":{"b":1}}}"#;
        assert_eq!(
            plain(src, 80),
            [
                "{",
                r#"  "name": "lsd","#,
                r#"  "tags": ["cli", "rust"],"#,
                r#"  "pos": { "x": 1, "y": 2 },"#,
                r#"  "deps": {"#,
                r#"    "a": { "b": 1 }"#,
                "  }",
                "}",
            ]
        );
    }

    #[test]
    fn fills_long_number_arrays() {
        let src = format!(
            "[{}]",
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let lines = plain(&src, 24);
        assert_eq!(lines[0], "[");
        assert_eq!(lines[1], "  1, 2, 3, 4, 5, 6, 7,");
        assert_eq!(lines.last().unwrap(), "]");
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(render("{ nope", 80).is_none());
    }
}
