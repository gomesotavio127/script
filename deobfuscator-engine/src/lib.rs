use std::alloc::{alloc as raw_alloc, dealloc as raw_dealloc, Layout};
use std::slice;
use std::str;

const VERSION: &str = "0.1.0";
static mut LAST_OUTPUT_LEN: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(size: usize) -> *mut u8 {
    let layout = Layout::array::<u8>(size.max(1)).unwrap();
    unsafe { raw_alloc(layout) }
}

#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, size: usize) {
    if !ptr.is_null() {
        if let Ok(layout) = Layout::array::<u8>(size.max(1)) {
            raw_dealloc(ptr, layout);
        }
    }
}

#[no_mangle]
pub extern "C" fn engine_run(ptr: *const u8, len: usize, mode: u32) -> *mut u8 {
    if ptr.is_null() || len > 32 * 1024 * 1024 {
        return output_ptr(error_json("Input is empty or exceeds the 32 MiB engine limit."));
    }
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    let response = if mode == 1 {
        inspect_bytecode(bytes)
    } else {
        match str::from_utf8(bytes) {
            Ok(source) => analyze_source(source),
            Err(_) => {
                let source = String::from_utf8_lossy(bytes);
                analyze_source(&source)
            }
        }
    };
    output_ptr(response)
}

#[no_mangle]
pub unsafe extern "C" fn result_len() -> usize {
    LAST_OUTPUT_LEN
}

#[no_mangle]
pub unsafe extern "C" fn free_result(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        let slice_ptr = std::ptr::slice_from_raw_parts_mut(ptr, len);
        drop(Box::from_raw(slice_ptr));
    }
}

fn output_ptr(response: String) -> *mut u8 {
    let boxed = response.into_bytes().into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    unsafe { LAST_OUTPUT_LEN = len; }
    ptr
}

fn error_json(message: &str) -> String {
    format!(
        "{{\"ok\":false,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"error\":\"{}\"}}",
        VERSION,
        json_escape(message)
    )
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn quote_json_array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|s| format!("\"{}\"", json_escape(s))).collect();
    format!("[{}]", items.join(","))
}

fn analyze_source(source: &str) -> String {
    let (output, comments_removed, strings_normalized, long_strings_normalized, mut issues) =
        normalize_source(source);
    let (delimiter_ok, delimiter_issues) = check_delimiters(&output);
    issues.extend(delimiter_issues);
    let findings = source_findings(source);
    let bytes = source.as_bytes().len();
    let chars = source.chars().count();
    let summary = if issues.is_empty() {
        "Conservative source normalization completed. This is not a proof of full syntactic validity."
    } else {
        "Normalization completed with warnings; review the affected areas before relying on the output."
    };

    format!(
        "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"mode\":\"source\",\"summary\":\"{}\",\"output\":\"{}\",\"metrics\":{{\"bytes\":{},\"characters\":{},\"comments_removed\":{},\"strings_normalized\":{},\"long_strings_normalized\":{},\"delimiter_check\":\"{}\"}},\"findings\":{},\"warnings\":{},\"scope\":\"No script execution, no model calls, no speculative rewriting. Bytecode decompilation is a separate capability.\"}}",
        VERSION,
        json_escape(summary),
        json_escape(&output),
        bytes,
        chars,
        comments_removed,
        strings_normalized,
        long_strings_normalized,
        if delimiter_ok { "balanced" } else { "mismatch" },
        quote_json_array(&findings),
        quote_json_array(&issues)
    )
}

fn normalize_source(source: &str) -> (String, usize, usize, usize, Vec<String>) {
    let b = source.as_bytes();
    let mut i = 0;
    let mut out = String::with_capacity(source.len());
    let mut comments = 0;
    let mut strings = 0;
    let mut long_strings = 0;
    let mut issues = Vec::new();

    while i < b.len() {
        // Lua/Luau line and long comments.
        if b[i] == b'-' && i + 1 < b.len() && b[i + 1] == b'-' {
            comments += 1;
            if let Some((content_start, close)) = long_opener(b, i + 2) {
                if let Some(end) = find_bytes(b, content_start, &close) {
                    preserve_newlines(b, i, end + close.len(), &mut out);
                    i = end + close.len();
                } else {
                    preserve_newlines(b, i, b.len(), &mut out);
                    issues.push("An unterminated long comment was found; its text was removed only up to end-of-file.".to_string());
                    i = b.len();
                }
            } else {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' { i += 1; }
                // Keep the line ending so source locations remain roughly stable.
            }
            continue;
        }

        // Quoted Lua strings. Unknown/invalid escapes are preserved verbatim.
        if b[i] == b'"' || b[i] == b'\'' {
            match decode_quoted(b, i) {
                Ok((end, decoded)) => {
                    let normalized = quote_lua_bytes(&decoded);
                    let original = &source[i..end];
                    if normalized != original { strings += 1; }
                    out.push_str(&normalized);
                    i = end;
                }
                Err(reason) => {
                    issues.push(format!("String near byte {} was preserved unchanged: {}.", i, reason));
                    let end = find_quoted_end(b, i);
                    out.push_str(&source[i..end]);
                    i = end;
                }
            }
            continue;
        }

        // Lua long-bracket strings; the initial newline is discarded by Lua semantics.
        if b[i] == b'[' {
            if let Some((content_start, close)) = long_opener(b, i) {
                if let Some(end) = find_bytes(b, content_start, &close) {
                    let mut content_start_actual = content_start;
                    if content_start_actual < end && b[content_start_actual] == b'\n' {
                        content_start_actual += 1;
                    } else if content_start_actual + 1 < end
                        && b[content_start_actual] == b'\r'
                        && b[content_start_actual + 1] == b'\n'
                    {
                        content_start_actual += 2;
                    }
                    let decoded = &b[content_start_actual..end];
                    out.push_str(&quote_lua_bytes(decoded));
                    long_strings += 1;
                    i = end + close.len();
                    continue;
                } else {
                    issues.push(format!("Long-bracket string near byte {} is not terminated; it was preserved.", i));
                }
            }
        }

        // Luau interpolated strings are left untouched: nested interpolation needs a parser.
        if b[i] == 96 {
            let end = find_backtick_end(b, i);
            out.push_str(&source[i..end]);
            if end == b.len() && (b.len() == i + 1 || b[b.len() - 1] != 96) {
                issues.push(format!("Backtick string near byte {} may be unterminated; it was preserved.", i));
            }
            i = end;
            continue;
        }

        let ch_len = source[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&source[i..i + ch_len]);
        i += ch_len;
    }
    (out, comments, strings, long_strings, issues)
}

fn long_opener(b: &[u8], start: usize) -> Option<(usize, Vec<u8>)> {
    if start >= b.len() || b[start] != b'[' { return None; }
    let mut j = start + 1;
    while j < b.len() && b[j] == b'=' { j += 1; }
    if j < b.len() && b[j] == b'[' {
        let eq_count = j - start - 1;
        let mut close = Vec::with_capacity(eq_count + 2);
        close.push(b']');
        for _ in 0..eq_count { close.push(b'='); }
        close.push(b']');
        Some((j + 1, close))
    } else {
        None
    }
}

fn find_bytes(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() { return Some(from); }
    if from > b.len() || needle.len() > b.len().saturating_sub(from) { return None; }
    (from..=b.len() - needle.len()).find(|&i| &b[i..i + needle.len()] == needle)
}

fn preserve_newlines(b: &[u8], start: usize, end: usize, out: &mut String) {
    for byte in &b[start..end] {
        if *byte == b'\n' { out.push('\n'); }
        else if *byte == b'\r' { out.push('\r'); }
    }
}

fn find_quoted_end(b: &[u8], start: usize) -> usize {
    let quote = b[start];
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == b'\\' { i = (i + 2).min(b.len()); continue; }
        if b[i] == quote { return i + 1; }
        if b[i] == b'\n' || b[i] == b'\r' { return i; }
        i += 1;
    }
    b.len()
}

fn decode_quoted(b: &[u8], start: usize) -> Result<(usize, Vec<u8>), &'static str> {
    let quote = *b.get(start).ok_or("missing opening quote")?;
    let mut i = start + 1;
    let mut decoded = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == quote { return Ok((i + 1, decoded)); }
        if c == b'\n' || c == b'\r' { return Err("raw newline inside a quoted string"); }
        if c != b'\\' {
            decoded.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if i >= b.len() { return Err("escape at end-of-file"); }
        let e = b[i];
        i += 1;
        match e {
            b'a' => decoded.push(7),
            b'b' => decoded.push(8),
            b'f' => decoded.push(12),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'v' => decoded.push(11),
            b'\\' => decoded.push(b'\\'),
            b'"' => decoded.push(b'"'),
            b'\'' => decoded.push(b'\''),
            b'\n' => decoded.push(b'\n'),
            b'\r' => {
                if i < b.len() && b[i] == b'\n' { i += 1; }
                decoded.push(b'\n');
            }
            b'z' => {
                while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b'\n' || b[i] == b'\r' || b[i] == 11 || b[i] == 12) {
                    i += 1;
                }
            }
            b'x' => {
                if i + 1 >= b.len() { return Err("incomplete hexadecimal escape"); }
                let hi = hex_value(b[i]).ok_or("invalid hexadecimal escape")?;
                let lo = hex_value(b[i + 1]).ok_or("invalid hexadecimal escape")?;
                decoded.push((hi << 4) | lo);
                i += 2;
            }
            b'u' => {
                if i >= b.len() || b[i] != b'{' { return Err("unsupported Unicode escape"); }
                i += 1;
                let digits_start = i;
                let mut value: u32 = 0;
                while i < b.len() && b[i] != b'}' {
                    let digit = hex_value(b[i]).ok_or("invalid Unicode escape")? as u32;
                    value = value.checked_mul(16).and_then(|v| v.checked_add(digit)).ok_or("Unicode escape overflow")?;
                    i += 1;
                    if i - digits_start > 8 { return Err("Unicode escape too long"); }
                }
                if i >= b.len() || i == digits_start { return Err("unterminated Unicode escape"); }
                i += 1;
                let ch = char::from_u32(value).ok_or("invalid Unicode code point")?;
                let mut buf = [0u8; 4];
                decoded.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            d if d.is_ascii_digit() => {
                let mut value = (d - b'0') as u16;
                let mut count = 1;
                while count < 3 && i < b.len() && b[i].is_ascii_digit() {
                    value = value * 10 + (b[i] - b'0') as u16;
                    i += 1;
                    count += 1;
                }
                if value > 255 { return Err("decimal escape is greater than 255"); }
                decoded.push(value as u8);
            }
            _ => return Err("unknown escape sequence"),
        }
    }
    Err("unterminated string literal")
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn quote_lua_bytes(bytes: &[u8]) -> String {
    let valid = str::from_utf8(bytes).ok();
    let mut out = String::from("\"");
    if let Some(text) = valid {
        for ch in text.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{0007}' => out.push_str("\\a"),
                '\u{0008}' => out.push_str("\\b"),
                '\u{000c}' => out.push_str("\\f"),
                '\u{000b}' => out.push_str("\\v"),
                c if (c as u32) < 0x20 || c == '\u{007f}' => out.push_str(&format!("\\{:03}", c as u32)),
                c => out.push(c),
            }
        }
    } else {
        for byte in bytes {
            match *byte {
                b'"' => out.push_str("\\\""),
                b'\\' => out.push_str("\\\\"),
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                b'\x07' => out.push_str("\\a"),
                b'\x08' => out.push_str("\\b"),
                b'\x0c' => out.push_str("\\f"),
                b'\x0b' => out.push_str("\\v"),
                0x20..=0x7e => out.push(*byte as char),
                b => out.push_str(&format!("\\{:03}", b)),
            }
        }
    }
    out.push('"');
    out
}

fn find_backtick_end(b: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    let mut escaped = false;
    while i < b.len() {
        let c = b[i];
        if escaped { escaped = false; i += 1; continue; }
        if c == b'\\' { escaped = true; i += 1; continue; }
        if c == 96 { return i + 1; }
        i += 1;
    }
    b.len()
}

fn check_delimiters(source: &str) -> (bool, Vec<String>) {
    let b = source.as_bytes();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut issues = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' || b[i] == b'\'' {
            i = find_quoted_end(b, i);
            continue;
        }
        if b[i] == 96 {
            i = find_backtick_end(b, i);
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => stack.push((b[i], i)),
            b')' | b']' | b'}' => {
                let expected = match b[i] { b')' => b'(', b']' => b'[', _ => b'{' };
                if let Some((open, _)) = stack.pop() {
                    if open != expected {
                        issues.push(format!("Closing delimiter '{}' near byte {} does not match the previous opener.", b[i] as char, i));
                    }
                } else {
                    issues.push(format!("Unexpected closing delimiter '{}' near byte {}.", b[i] as char, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    for (open, pos) in stack.iter().rev() {
        issues.push(format!("Opening delimiter '{}' near byte {} has no matching close.", *open as char, pos));
    }
    (issues.is_empty(), issues)
}

fn source_findings(source: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let patterns: [(&str, &str); 8] = [
        ("loadstring", "Dynamic code loading pattern found; inspect where the loaded text comes from."),
        ("string.char", "Character construction pattern found; string-decoder analysis may be useful."),
        ("string.byte", "Byte-wise string processing pattern found."),
        ("bit32.", "Bitwise operations found; this may be used in a VM or decoder."),
        ("getfenv", "Environment access found; semantics may depend on the runtime."),
        ("setfenv", "Environment mutation found; semantics may depend on the runtime."),
        ("while true", "Unbounded loop pattern found; inspect control flow manually."),
        ("debug.info", "Debug/introspection API found; this may be runtime-specific."),
    ];
    let lower = source.to_ascii_lowercase();
    for (needle, finding) in patterns {
        if lower.contains(&needle.to_ascii_lowercase()) {
            findings.push(finding.to_string());
        }
    }
    let punctuation = source.chars().filter(|c| *c == ';' || *c == ',').count();
    let letters = source.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if source.len() > 20_000 && punctuation > 4_000 && letters > 0 && punctuation / letters.max(1) > 2 {
        findings.push("High punctuation density: possible generated or heavily transformed source (heuristic only).".to_string());
    }
    if findings.is_empty() {
        findings.push("No targeted indicators matched. This does not prove the source is unobfuscated.".to_string());
    }
    findings
}

fn inspect_bytecode(bytes: &[u8]) -> String {
    let size = bytes.len();
    if size >= 4 && &bytes[0..4] == b"\x1bLua" {
        let version = bytes.get(4).copied().unwrap_or(0);
        let (name, supported_header) = match version {
            0x51 => ("Lua 5.1", size >= 12),
            0x52 => ("Lua 5.2", size >= 12),
            0x53 => ("Lua 5.3", size >= 12),
            0x54 => ("Lua 5.4", size >= 12),
            _ => ("Lua-family bytecode with unknown version", size >= 6),
        };
        let format = bytes.get(5).copied().unwrap_or(255);
        let mut warnings = Vec::new();
        warnings.push("This v1 engine inspects the binary header only; it does not yet decompile instructions or recover function bodies.".to_string());
        if !supported_header { warnings.push("The file is shorter than the minimum expected header.".to_string()); }
        if format != 0 { warnings.push(format!("Non-zero chunk format byte {} is unusual; compatibility is not guaranteed.", format)); }
        let summary = if supported_header {
            format!("Recognized {} binary signature; header inspection only.", name)
        } else {
            "Recognized Lua bytecode signature, but the header appears incomplete.".to_string()
        };
        return format!(
            "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"mode\":\"bytecode\",\"format\":\"{}\",\"summary\":\"{}\",\"output\":\"\",\"metrics\":{{\"bytes\":{},\"header_detected\":{},\"chunk_format\":{}}},\"findings\":{},\"warnings\":{},\"scope\":\"Header inspection only; instruction decompilation is not implemented in v1.\"}}",
            VERSION, json_escape(name), json_escape(&summary), size, supported_header, format,
            quote_json_array(&vec!["Lua bytecode signature: 1B 4C 75 61".to_string()]),
            quote_json_array(&warnings)
        );
    }
    if bytes.starts_with(b"RSB1") {
        return format!(
            "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"mode\":\"bytecode\",\"format\":\"Roblox RSB1 container candidate\",\"summary\":\"Recognized RSB1 marker; the payload layout is not parsed in this version.\",\"output\":\"\",\"metrics\":{{\"bytes\":{},\"header_detected\":true}},\"findings\":{},\"warnings\":{},\"scope\":\"Container recognition only; no Luau instruction decompilation is implemented in v1.\"}}",
            VERSION, size,
            quote_json_array(&vec!["RSB1 marker found.".to_string()]),
            quote_json_array(&vec!["The marker alone does not prove that the payload is valid Luau bytecode.".to_string()])
        );
    }

    let sample = bytes.iter().take(16).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
    format!(
        "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"mode\":\"bytecode\",\"format\":\"unknown\",\"summary\":\"No supported Lua 5.1–5.4 signature was found.\",\"output\":\"\",\"metrics\":{{\"bytes\":{},\"header_detected\":false,\"first_bytes\":\"{}\"}},\"findings\":{},\"warnings\":{},\"scope\":\"Unknown binaries are not disassembled speculatively. Luau bytecode variants need an explicit format parser before reliable decompilation.\"}}",
        VERSION, size, json_escape(&sample),
        quote_json_array(&vec!["Header does not match the signatures implemented by this version.".to_string()]),
        quote_json_array(&vec!["This input may be Luau bytecode, compressed data, encrypted data, or another binary format.".to_string()])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_but_preserves_newlines() {
        let (out, comments, _, _, issues) = normalize_source("local x = 1 -- comment\n--[=[ block\ncomment ]=]\nprint(x)");
        assert_eq!(comments, 2);
        assert!(out.contains("local x = 1"));
        assert!(out.contains("print(x)"));
        assert!(issues.is_empty());
    }

    #[test]
    fn decodes_safe_string_escapes() {
        let (out, _, count, _, issues) = normalize_source("print('A\\065\\nB')");
        assert_eq!(count, 1);
        assert!(out.contains("print(\"AA\\nB\")"));
        assert!(issues.is_empty());
    }

    #[test]
    fn keeps_invalid_escapes_unchanged() {
        let (out, _, count, _, issues) = normalize_source("local x = '\\q'");
        assert_eq!(count, 0);
        assert!(out.contains("'\\q'"));
        assert!(!issues.is_empty());
    }

    #[test]
    fn identifies_lua_bytecode_header() {
        let mut b = b"\x1bLua".to_vec();
        b.extend_from_slice(&[0x51, 0, 1, 4, 4, 4, 8, 0]);
        let out = inspect_bytecode(&b);
        assert!(out.contains("Lua 5.1"));
        assert!(out.contains("header inspection only"));
    }
}
