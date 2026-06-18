//! Purpose: Deterministic JSON emitter for Rust-native command output.
//! Caller: Native command handlers in commands.rs that emit `--json` payloads.
//! Dependencies: std::io::{self, Write}.
//! Main Functions: write_indented (the public emitter); Value enum (Null/Bool/Number/String/Array/Object) carries the data.
//! Side Effects: Writes JSON bytes to the supplied writer; no global state.
//!
//! Byte-for-byte parity targets:
//! * Object fields keep declared order (we use an ordered `Vec<(String, Value)>`).
//! * `SetIndent("", "  ")` equivalent: two-space indent, `\n` newlines, trailing newline after each top-level value.
//! * HTML-sensitive bytes `<`, `>`, `&` are escaped to `<`, `>`, `&`.
//! * `"`, `\`, `\b`, `\f`, `\n`, `\r`, `\t` use their short escape forms.
//! * Other bytes below 0x20 escape as `\u00xx`.
//! * U+2028 and U+2029 escape as ` ` / ` ` for stable cross-host output.
//! * All other Unicode scalars above 0x1F emit as UTF-8.

use std::io::{self, Write};

#[derive(Debug, Clone)]
pub enum Value {
    String(String),
    Bool(bool),
    Number(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

pub fn write_indented<W: Write + ?Sized>(writer: &mut W, value: &Value) -> io::Result<()> {
    write_value(writer, value, 0)?;
    writer.write_all(b"\n")
}

fn write_value<W: Write + ?Sized>(writer: &mut W, value: &Value, depth: usize) -> io::Result<()> {
    match value {
        Value::Bool(true) => writer.write_all(b"true"),
        Value::Bool(false) => writer.write_all(b"false"),
        Value::Number(rendered_number) => writer.write_all(rendered_number.as_bytes()),
        Value::String(raw_string) => write_string(writer, raw_string),
        Value::Array(entries) => {
            if entries.is_empty() {
                writer.write_all(b"[]")
            } else {
                writer.write_all(b"[\n")?;
                for (index, entry) in entries.iter().enumerate() {
                    write_indent(writer, depth + 1)?;
                    write_value(writer, entry, depth + 1)?;
                    if index + 1 < entries.len() {
                        writer.write_all(b",")?;
                    }
                    writer.write_all(b"\n")?;
                }
                write_indent(writer, depth)?;
                writer.write_all(b"]")
            }
        }
        Value::Object(fields) => {
            if fields.is_empty() {
                writer.write_all(b"{}")
            } else {
                writer.write_all(b"{\n")?;
                for (index, (field_key, field_value)) in fields.iter().enumerate() {
                    write_indent(writer, depth + 1)?;
                    write_string(writer, field_key)?;
                    writer.write_all(b": ")?;
                    write_value(writer, field_value, depth + 1)?;
                    if index + 1 < fields.len() {
                        writer.write_all(b",")?;
                    }
                    writer.write_all(b"\n")?;
                }
                write_indent(writer, depth)?;
                writer.write_all(b"}")
            }
        }
    }
}

fn write_indent<W: Write + ?Sized>(writer: &mut W, depth: usize) -> io::Result<()> {
    for _ in 0..depth {
        writer.write_all(b"  ")?;
    }
    Ok(())
}

fn write_string<W: Write + ?Sized>(writer: &mut W, raw_string: &str) -> io::Result<()> {
    writer.write_all(b"\"")?;
    let bytes = raw_string.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte < 0x20
            || byte == b'"'
            || byte == b'\\'
            || byte == b'<'
            || byte == b'>'
            || byte == b'&'
        {
            if start < index {
                writer.write_all(&bytes[start..index])?;
            }
            match byte {
                b'"' => writer.write_all(b"\\\"")?,
                b'\\' => writer.write_all(b"\\\\")?,
                b'\n' => writer.write_all(b"\\n")?,
                b'\r' => writer.write_all(b"\\r")?,
                b'\t' => writer.write_all(b"\\t")?,
                0x08 => writer.write_all(b"\\b")?,
                0x0c => writer.write_all(b"\\f")?,
                b'<' => writer.write_all(b"\\u003c")?,
                b'>' => writer.write_all(b"\\u003e")?,
                b'&' => writer.write_all(b"\\u0026")?,
                other => {
                    write!(writer, "\\u{:04x}", other)?;
                }
            }
            index += 1;
            start = index;
            continue;
        }
        if byte == 0xE2
            && index + 2 < bytes.len()
            && bytes[index + 1] == 0x80
            && (bytes[index + 2] == 0xA8 || bytes[index + 2] == 0xA9)
        {
            if start < index {
                writer.write_all(&bytes[start..index])?;
            }
            if bytes[index + 2] == 0xA8 {
                writer.write_all(b"\\u2028")?;
            } else {
                writer.write_all(b"\\u2029")?;
            }
            index += 3;
            start = index;
            continue;
        }
        index += 1;
    }
    if start < bytes.len() {
        writer.write_all(&bytes[start..])?;
    }
    writer.write_all(b"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(value: &Value) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        write_indented(&mut buffer, value).expect("render succeeds");
        String::from_utf8(buffer).expect("utf-8")
    }

    #[test]
    fn renders_empty_object() {
        assert_eq!(render(&Value::Object(Vec::new())), "{}\n");
    }

    #[test]
    fn renders_two_space_indent_and_trailing_newline() {
        let value = Value::Object(vec![
            ("a".into(), Value::String("1".into())),
            ("b".into(), Value::Number("2".into())),
        ]);
        let rendered = render(&value);
        assert_eq!(rendered, "{\n  \"a\": \"1\",\n  \"b\": 2\n}\n");
    }

    #[test]
    fn escapes_html_sensitive_bytes() {
        let value = Value::String("<a>&b</a>".into());
        assert_eq!(
            render(&value),
            "\"\\u003ca\\u003e\\u0026b\\u003c/a\\u003e\"\n"
        );
    }

    #[test]
    fn escapes_control_characters() {
        let value = Value::String("\x01\n\t".into());
        assert_eq!(render(&value), "\"\\u0001\\n\\t\"\n");
    }

    #[test]
    fn escapes_line_and_paragraph_separators() {
        let value = Value::String("line\u{2028}para\u{2029}end".into());
        assert_eq!(render(&value), "\"line\\u2028para\\u2029end\"\n");
    }

    #[test]
    fn passes_through_non_ascii_utf8() {
        let value = Value::String("café".into());
        assert_eq!(render(&value), "\"café\"\n");
    }
}
