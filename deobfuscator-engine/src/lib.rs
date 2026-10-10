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
    let (normalized, comments_removed, strings_normalized, long_strings_normalized, mut issues) =
        normalize_source(source);
    let mut output = normalized;
    let mut expressions_folded = 0usize;
    let mut static_calls_folded = 0usize;
    let mut locals_propagated = 0usize;
    let mut branches_removed = 0usize;
    let mut converged = false;

    // Deterministic fixed-point passes. This never executes the input.
    for _ in 0..8 {
        let before = output.clone();
        let (next, calls) = fold_static_calls(&output);
        static_calls_folded += calls;
        let (next, expressions) = fold_constant_expressions(&next);
        expressions_folded += expressions;
        let (next, locals) = propagate_local_constants(&next);
        locals_propagated += locals;
        let (next, branches) = simplify_constant_control_flow(&next);
        branches_removed += branches;
        output = next;
        if output == before {
            converged = true;
            break;
        }
    }

    if !converged {
        issues.push("Static passes reached the eight-round safety limit; the output may contain further recoverable expressions.".to_string());
    }
    let (delimiter_ok, delimiter_issues) = check_delimiters(&output);
    issues.extend(delimiter_issues);
    let findings = source_findings(source);
    let lower = source.to_ascii_lowercase();
    let looks_like_loader = lower.contains("loadstring")
        && (lower.contains("httpget") || lower.contains("request(") || lower.contains("http_request"));
    if looks_like_loader {
        issues.push("This input appears to be a loader. Network retrieval and script execution are intentionally disabled; provide the downloaded payload as a local file to analyze its contents.".to_string());
    }

    let transformations = expressions_folded + static_calls_folded + locals_propagated + branches_removed;
    let bytes = source.as_bytes().len();
    let chars = source.chars().count();
    let format_name = if looks_like_loader {
        "Lua/Luau dynamic loader"
    } else {
        "Lua/Luau source"
    };
    let summary = if transformations == 0 {
        "Analysis completed; no supported static transformation was applicable. The output is not claimed to be deobfuscated."
            .to_string()
    } else {
        format!(
            "Static recovery applied {} transformation(s). Review warnings; complex VM-based obfuscation may remain.",
            transformations
        )
    };

    format!(
        "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"mode\":\"source\",\"format\":\"{}\",\"summary\":\"{}\",\"output\":\"{}\",\"metrics\":{{\"bytes\":{},\"characters\":{},\"output_bytes\":{},\"comments_removed\":{},\"strings_normalized\":{},\"long_strings_normalized\":{},\"constant_expressions_folded\":{},\"static_calls_folded\":{},\"local_constants_propagated\":{},\"constant_branches_removed\":{},\"transformations_total\":{},\"analysis_round_limit\":8,\"delimiter_check\":\"{}\"}},\"findings\":{},\"warnings\":{},\"scope\":\"Deterministic static source recovery; no script execution, network access, or model calls. Only understood constant expressions, selected pure string calls, simple local constants, and provably constant branches are rewritten. Unsupported syntax is preserved.\"}}",
        VERSION,
        json_escape(format_name),
        json_escape(&summary),
        json_escape(&output),
        bytes,
        chars,
        output.as_bytes().len(),
        comments_removed,
        strings_normalized,
        long_strings_normalized,
        expressions_folded,
        static_calls_folded,
        locals_propagated,
        branches_removed,
        transformations,
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

    // The real decoder runs first. It returns source only after the upstream
    // parser and lifter accept the complete chunk; failed inputs are never
    // presented as successfully decompiled code.
    match luau_core::decompile(bytes) {
        Ok(source) => {
            let bytecode_version = luau_core::bytecode_version(bytes)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let lines = source.lines().count();
            let summary = format!(
                "Luau bytecode decoded successfully (bytecode version {}). Review the recovered source; original comments and names may not be recoverable.",
                bytecode_version
            );
            return format!(
                "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust + luau-core\",\"version\":\"{}\",\"mode\":\"bytecode\",\"format\":\"Luau bytecode\",\"summary\":\"{}\",\"output\":\"{}\",\"metrics\":{{\"bytes\":{},\"bytecode_version\":\"{}\",\"output_characters\":{},\"output_lines\":{}}},\"findings\":{},\"warnings\":{},\"scope\":\"Actual bytecode parsing and decompilation performed by the MIT-licensed luau-core Rust engine. This is recovered source, not a guarantee of original text identity.\"}}",
                VERSION,
                json_escape(&summary),
                json_escape(&source),
                size,
                json_escape(&bytecode_version),
                source.chars().count(),
                lines,
                quote_json_array(&vec![
                    "Luau bytecode was parsed and lifted into source code.".to_string()
                ]),
                quote_json_array(&vec![
                    "Decompilation cannot restore comments and identifiers discarded during compilation.".to_string()
                ])
            );
        }
        Err(error) => {
            let message = format!("{:#}", error);
            let sample = bytes.iter().take(16).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
            let version = luau_core::bytecode_version(bytes)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let (format_name, header_detected) = if bytes.starts_with(b"\x1bLua") {
                ("Lua-family bytecode (not Luau)", true)
            } else if bytes.starts_with(b"RSB1") {
                ("Roblox RSB1 container candidate", true)
            } else if luau_core::bytecode_version(bytes).is_some() {
                ("Luau bytecode header detected, but decoding failed", true)
            } else {
                ("unknown", false)
            };
            let summary = if header_detected {
                "A binary signature was detected, but the Luau decoder rejected the input; no source was fabricated."
            } else {
                "The file could not be decoded as a supported Luau bytecode chunk."
            };
            format!(
                "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust + luau-core\",\"version\":\"{}\",\"mode\":\"bytecode\",\"format\":\"{}\",\"summary\":\"{}\",\"output\":\"\",\"metrics\":{{\"bytes\":{},\"header_detected\":{},\"bytecode_version\":\"{}\",\"first_bytes\":\"{}\"}},\"findings\":{},\"warnings\":{},\"scope\":\"No source output is produced when decoding fails. The upstream core currently targets standard Roblox Luau bytecode versions 3–9 and rejects uncertain formats rather than guessing.\"}}",
                VERSION,
                json_escape(format_name),
                json_escape(summary),
                size,
                header_detected,
                json_escape(&version),
                json_escape(&sample),
                quote_json_array(&vec![
                    "The decoder did not accept this input; it may be another VM, an encrypted/compressed wrapper, or an unsupported bytecode version.".to_string()
                ]),
                quote_json_array(&vec![
                    format!("Decoder detail: {}", message)
                ])
            )
        }
    }
}


#[derive(Clone, Debug, PartialEq)]
enum ConstValue {
    Nil,
    Bool(bool),
    Number(f64),
    String(Vec<u8>),
}

impl ConstValue {
    fn truthy(&self) -> bool {
        !matches!(self, ConstValue::Nil | ConstValue::Bool(false))
    }

    fn to_lua(&self) -> String {
        match self {
            ConstValue::Nil => "nil".to_string(),
            ConstValue::Bool(v) => v.to_string(),
            ConstValue::Number(v) => {
                if !v.is_finite() { return "nil".to_string(); }
                if v.fract() == 0.0 && v.abs() < 9_007_199_254_740_991.0 {
                    format!("{:.0}", v)
                } else {
                    let rendered = v.to_string();
                    if rendered == "-0" { "0".to_string() } else { rendered }
                }
            }
            ConstValue::String(v) => quote_lua_bytes(v),
        }
    }

    fn as_number(&self) -> Option<f64> {
        match self { ConstValue::Number(v) if v.is_finite() => Some(*v), _ => None }
    }

    fn as_string(&self) -> Option<&[u8]> {
        match self { ConstValue::String(v) => Some(v.as_slice()), _ => None }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Ident,
    Number,
    String,
    Symbol,
    Opaque,
}

#[derive(Clone, Debug)]
struct Token {
    start: usize,
    end: usize,
    line: usize,
    text: String,
    kind: TokenKind,
}

fn lex_source(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;

    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            if bytes[i] == b'\n' { line += 1; }
            i += 1;
            continue;
        }

        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            let start = i;
            if let Some((content_start, close)) = long_opener(bytes, i + 2) {
                if let Some(end) = find_bytes(bytes, content_start, &close) {
                    i = end + close.len();
                } else {
                    i = bytes.len();
                }
            } else {
                while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' { i += 1; }
            }
            line += bytes[start..i].iter().filter(|b| **b == b'\n').count();
            continue;
        }

        let start = i;
        let token_line = line;
        let kind;
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            i = find_quoted_end(bytes, i);
            kind = TokenKind::String;
        } else if bytes[i] == 96 {
            i = find_backtick_end(bytes, i);
            kind = TokenKind::Opaque;
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
            kind = TokenKind::Ident;
        } else if bytes[i].is_ascii_digit()
            || (bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            i = scan_number(bytes, i);
            kind = TokenKind::Number;
        } else if bytes[i] >= 0x80 {
            let n = source[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            i += n;
            kind = TokenKind::Opaque;
        } else {
            let rest = &bytes[i..];
            let width = if rest.starts_with(b"...") || rest.starts_with(b"..=") || rest.starts_with(b"<<=") || rest.starts_with(b">>=") {
                3
            } else if rest.starts_with(b"..") || rest.starts_with(b"//") || rest.starts_with(b"==")
                || rest.starts_with(b"~=") || rest.starts_with(b"<=") || rest.starts_with(b">=")
                || rest.starts_with(b"+=") || rest.starts_with(b"-=") || rest.starts_with(b"*=")
                || rest.starts_with(b"/=") || rest.starts_with(b"%=") || rest.starts_with(b"^=")
                || rest.starts_with(b"<<") || rest.starts_with(b">>") || rest.starts_with(b"::")
                || rest.starts_with(b"->")
            {
                2
            } else { 1 };
            i = (i + width).min(bytes.len());
            kind = TokenKind::Symbol;
        }

        line += bytes[start..i].iter().filter(|b| **b == b'\n').count();
        if let Some(raw) = source.get(start..i) {
            tokens.push(Token { start, end: i, line: token_line, text: raw.to_string(), kind });
        }
    }
    tokens
}

fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    if i + 1 < bytes.len() && bytes[i] == b'0' && matches!(bytes[i + 1], b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
        let base = bytes[i + 1].to_ascii_lowercase();
        i += 2;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
        if base == b'x' && i < bytes.len() && bytes[i] == b'.' && !(i + 1 < bytes.len() && bytes[i + 1] == b'.') {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') { i += 1; }
            if i < bytes.len() && matches!(bytes[i], b'p' | b'P') {
                i += 1;
                if i < bytes.len() && matches!(bytes[i], b'+' | b'-') { i += 1; }
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') { i += 1; }
            }
        }
        return i;
    }

    if bytes[i] == b'.' { i += 1; }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') { i += 1; }
    if i < bytes.len() && bytes[i] == b'.' && !(i + 1 < bytes.len() && bytes[i + 1] == b'.') {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') { i += 1; }
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        let exponent_start = i;
        i += 1;
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') { i += 1; }
        let digits_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') { i += 1; }
        if digits_start == i { i = exponent_start; }
    }
    i
}

fn parse_number(raw: &str) -> Option<f64> {
    let clean = raw.replace('_', "");
    let lower = clean.to_ascii_lowercase();
    if let Some(digits) = lower.strip_prefix("0x") {
        if digits.contains('p') || digits.contains('.') { return None; }
        return u128::from_str_radix(digits, 16).ok().map(|v| v as f64);
    }
    if let Some(digits) = lower.strip_prefix("0b") {
        return u128::from_str_radix(digits, 2).ok().map(|v| v as f64);
    }
    if let Some(digits) = lower.strip_prefix("0o") {
        return u128::from_str_radix(digits, 8).ok().map(|v| v as f64);
    }
    clean.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn constant_from_token(source: &str, token: &Token) -> Option<ConstValue> {
    match token.kind {
        TokenKind::Number => parse_number(&token.text).map(ConstValue::Number),
        TokenKind::String => {
            let bytes = source.as_bytes();
            match decode_quoted(bytes, token.start) {
                Ok((end, decoded)) if end == token.end => Some(ConstValue::String(decoded)),
                _ => None,
            }
        }
        TokenKind::Ident => match token.text.as_str() {
            "true" => Some(ConstValue::Bool(true)),
            "false" => Some(ConstValue::Bool(false)),
            "nil" => Some(ConstValue::Nil),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct ExprNode {
    start: usize,
    end: usize,
    value: Option<ConstValue>,
    has_operator: bool,
    children: Vec<ExprNode>,
}

fn binary_precedence(op: &str) -> Option<(u8, bool)> {
    match op {
        "or" => Some((1, false)),
        "and" => Some((2, false)),
        "==" | "~=" | "<" | ">" | "<=" | ">=" => Some((3, false)),
        "|" => Some((4, false)),
        "~" => Some((5, false)),
        "&" => Some((6, false)),
        "<<" | ">>" => Some((7, false)),
        ".." => Some((8, true)),
        "+" | "-" => Some((9, false)),
        "*" | "/" | "//" | "%" => Some((10, false)),
        "^" => Some((12, true)),
        _ => None,
    }
}

fn integer_value(value: &ConstValue) -> Option<i64> {
    match value {
        ConstValue::Number(n) if n.is_finite() && n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 => Some(*n as i64),
        _ => None,
    }
}

fn value_eq(left: &ConstValue, right: &ConstValue) -> bool {
    match (left, right) {
        (ConstValue::Nil, ConstValue::Nil) => true,
        (ConstValue::Bool(a), ConstValue::Bool(b)) => a == b,
        (ConstValue::Number(a), ConstValue::Number(b)) => a == b,
        (ConstValue::String(a), ConstValue::String(b)) => a == b,
        _ => false,
    }
}

fn concat_bytes(value: &ConstValue) -> Option<Vec<u8>> {
    match value {
        ConstValue::String(v) => Some(v.clone()),
        ConstValue::Number(v) if v.is_finite() => Some(v.to_string().into_bytes()),
        _ => None,
    }
}

fn eval_unary(op: &str, value: &ConstValue) -> Option<ConstValue> {
    match op {
        "not" => Some(ConstValue::Bool(!value.truthy())),
        "+" => value.as_number().map(ConstValue::Number),
        "-" => value.as_number().map(|n| ConstValue::Number(-n)).filter(|v| v.as_number().is_some()),
        "~" => integer_value(value).map(|n| ConstValue::Number((!(n as u32)) as i32 as f64)),
        "#" => value.as_string().map(|s| ConstValue::Number(s.len() as f64)),
        _ => None,
    }
}

fn eval_binary(op: &str, left: &ConstValue, right: &ConstValue) -> Option<ConstValue> {
    if op == "and" { return Some(if left.truthy() { right.clone() } else { left.clone() }); }
    if op == "or" { return Some(if left.truthy() { left.clone() } else { right.clone() }); }
    match op {
        "==" => return Some(ConstValue::Bool(value_eq(left, right))),
        "~=" => return Some(ConstValue::Bool(!value_eq(left, right))),
        "<" | ">" | "<=" | ">=" => {
            let cmp = match (left, right) {
                (ConstValue::Number(a), ConstValue::Number(b)) => a.partial_cmp(b),
                (ConstValue::String(a), ConstValue::String(b)) => Some(a.cmp(b)),
                _ => None,
            }?;
            let result = match op {
                "<" => cmp.is_lt(), ">" => cmp.is_gt(), "<=" => !cmp.is_gt(), ">=" => !cmp.is_lt(), _ => false,
            };
            return Some(ConstValue::Bool(result));
        }
        ".." => {
            let mut result = concat_bytes(left)?;
            result.extend_from_slice(&concat_bytes(right)?);
            return Some(ConstValue::String(result));
        }
        "|" | "~" | "&" | "<<" | ">>" => {
            let a = integer_value(left)? as u32;
            let b = integer_value(right)? as u32;
            let result = match op {
                "|" => a | b,
                "~" => a ^ b,
                "&" => a & b,
                "<<" => a.wrapping_shl(b & 31),
                ">>" => a.wrapping_shr(b & 31),
                _ => return None,
            };
            return Some(ConstValue::Number(result as i32 as f64));
        }
        _ => {}
    }
    let a = left.as_number()?;
    let b = right.as_number()?;
    let value = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" if b != 0.0 => a / b,
        "//" if b != 0.0 => (a / b).floor(),
        "%" if b != 0.0 => a - (a / b).floor() * b,
        "^" => a.powf(b),
        _ => return None,
    };
    if value.is_finite() { Some(ConstValue::Number(value)) } else { None }
}

fn parse_primary(
    source: &str,
    tokens: &[Token],
    pos: &mut usize,
    limit: usize,
    depth: usize,
) -> Option<ExprNode> {
    if *pos >= limit || depth > 128 { return None; }
    let index = *pos;
    let token = &tokens[index];

    if token.text == "(" {
        *pos += 1;
        let inner = parse_expr_bp(source, tokens, pos, limit, 1, depth + 1)?;
        if *pos >= limit || tokens[*pos].text != ")" {
            return Some(ExprNode {
                start: index, end: inner.end, value: None, has_operator: inner.has_operator,
                children: vec![inner],
            });
        }
        *pos += 1;
        return Some(ExprNode {
            start: index, end: *pos, value: inner.value.clone(), has_operator: inner.has_operator,
            children: vec![inner],
        });
    }

    if matches!(token.text.as_str(), "not" | "-" | "+" | "~" | "#") {
        let op = token.text.clone();
        *pos += 1;
        let min_prec = if op == "not" { 3 } else { 11 };
        let operand = parse_expr_bp(source, tokens, pos, limit, min_prec, depth + 1)?;
        let value = operand.value.as_ref().and_then(|v| eval_unary(&op, v));
        return Some(ExprNode {
            start: index, end: operand.end, value, has_operator: true, children: vec![operand],
        });
    }

    *pos += 1;
    let value = constant_from_token(source, token);
    Some(ExprNode {
        start: index, end: index + 1, value, has_operator: false, children: Vec::new(),
    })
}

fn parse_expr_bp(
    source: &str,
    tokens: &[Token],
    pos: &mut usize,
    limit: usize,
    min_prec: u8,
    depth: usize,
) -> Option<ExprNode> {
    if depth > 128 { return None; }
    let mut left = parse_primary(source, tokens, pos, limit, depth + 1)?;
    loop {
        if *pos >= limit { break; }
        let Some((precedence, right_associative)) = binary_precedence(&tokens[*pos].text) else { break; };
        if precedence < min_prec { break; }
        let op = tokens[*pos].text.clone();
        *pos += 1;
        let rhs_min = if right_associative { precedence } else { precedence + 1 };
        let right = match parse_expr_bp(source, tokens, pos, limit, rhs_min, depth + 1) {
            Some(value) => value,
            None => break,
        };
        let value = match (left.value.as_ref(), right.value.as_ref()) {
            (Some(a), Some(b)) => eval_binary(&op, a, b),
            _ => None,
        };
        let start = left.start;
        let end = right.end;
        left = ExprNode {
            start, end, value, has_operator: true, children: vec![left, right],
        };
    }
    Some(left)
}

fn eval_constant_range(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<ConstValue> {
    if start >= end || end > tokens.len() { return None; }
    let mut pos = start;
    let node = parse_expr_bp(source, tokens, &mut pos, end, 1, 0)?;
    if pos == end { node.value } else { None }
}

#[derive(Clone)]
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

fn apply_edits(source: &str, mut edits: Vec<Edit>) -> (String, usize) {
    if edits.is_empty() { return (source.to_string(), 0); }
    edits.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;
    let mut applied = 0usize;
    for edit in edits {
        if edit.start < cursor || edit.end > source.len() || edit.start > edit.end { continue; }
        if !source.is_char_boundary(edit.start) || !source.is_char_boundary(edit.end) { continue; }
        output.push_str(&source[cursor..edit.start]);
        output.push_str(&edit.replacement);
        cursor = edit.end;
        applied += 1;
    }
    output.push_str(&source[cursor..]);
    (output, applied)
}

fn collect_constant_edits(node: &ExprNode, source: &str, tokens: &[Token], edits: &mut Vec<Edit>) {
    if node.has_operator {
        if let Some(value) = node.value.as_ref() {
            if node.start < node.end && node.end <= tokens.len() {
                edits.push(Edit {
                    start: tokens[node.start].start,
                    end: tokens[node.end - 1].end,
                    replacement: value.to_lua(),
                });
                return;
            }
        }
    }
    for child in &node.children { collect_constant_edits(child, source, tokens, edits); }
}

fn fold_constant_expressions(source: &str) -> (String, usize) {
    let tokens = lex_source(source);
    let mut edits = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        let mut pos = i;
        if let Some(node) = parse_expr_bp(source, &tokens, &mut pos, tokens.len(), 1, 0) {
            if pos > i {
                collect_constant_edits(&node, source, &tokens, &mut edits);
                i = pos.max(i + 1);
                continue;
            }
        }
        i += 1;
    }
    apply_edits(source, edits)
}

fn find_matching_token(tokens: &[Token], open: usize, opener: &str, closer: &str) -> Option<usize> {
    if open >= tokens.len() || tokens[open].text != opener { return None; }
    let mut depth = 0usize;
    for i in open..tokens.len() {
        if tokens[i].text == opener { depth += 1; }
        else if tokens[i].text == closer {
            depth = depth.checked_sub(1)?;
            if depth == 0 { return Some(i); }
        }
    }
    None
}

fn split_arguments(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    if start >= end { return Vec::new(); }
    let mut args = Vec::new();
    let mut begin = start;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    for i in start..end {
        match tokens[i].text.as_str() {
            "(" => paren += 1, ")" => paren -= 1,
            "[" => square += 1, "]" => square -= 1,
            "{" => brace += 1, "}" => brace -= 1,
            "," if paren == 0 && square == 0 && brace == 0 => {
                args.push((begin, i));
                begin = i + 1;
            }
            _ => {}
        }
    }
    if begin < end { args.push((begin, end)); }
    args
}

fn bounded_lua_index(index: i64, len: usize, is_start: bool) -> usize {
    let len_i = len.min(i64::MAX as usize) as i64;
    let raw = if index < 0 { len_i + index + 1 } else { index };
    let clamped = if is_start { raw.clamp(1, len_i + 1) } else { raw.clamp(0, len_i) };
    if is_start { (clamped - 1).max(0) as usize } else { clamped.max(0) as usize }
}

fn fold_static_calls(source: &str) -> (String, usize) {
    let tokens = lex_source(source);
    let mut edits = Vec::new();
    let mut i = 0usize;
    while i + 3 < tokens.len() {
        let namespace = tokens[i].text.as_str();
        if !matches!(namespace, "string" | "table") || tokens[i + 1].text != "." || tokens[i + 3].text != "(" {
            i += 1;
            continue;
        }
        let function_name = tokens[i + 2].text.as_str();
        let Some(close) = find_matching_token(&tokens, i + 3, "(", ")") else { i += 1; continue; };
        let args = split_arguments(&tokens, i + 4, close);
        let mut result: Option<ConstValue> = None;

        if namespace == "string" && function_name == "char" {
            let mut bytes = Vec::new();
            let mut valid = true;
            for (a, b) in &args {
                match eval_constant_range(source, &tokens, *a, *b) {
                    Some(ConstValue::Number(n)) if n.is_finite() && n.fract() == 0.0 && (0.0..=255.0).contains(&n) => bytes.push(n as u8),
                    _ => { valid = false; break; }
                }
            }
            if valid && bytes.len() <= 65536 { result = Some(ConstValue::String(bytes)); }
        } else if namespace == "string" && function_name == "reverse" && args.len() == 1 {
            if let Some(ConstValue::String(mut value)) = eval_constant_range(source, &tokens, args[0].0, args[0].1) {
                value.reverse();
                result = Some(ConstValue::String(value));
            }
        } else if namespace == "string" && function_name == "sub" && (args.len() == 2 || args.len() == 3) {
            if let (Some(ConstValue::String(value)), Some(ConstValue::Number(first))) = (
                eval_constant_range(source, &tokens, args[0].0, args[0].1),
                eval_constant_range(source, &tokens, args[1].0, args[1].1),
            ) {
                let last = if args.len() == 3 {
                    match eval_constant_range(source, &tokens, args[2].0, args[2].1) {
                        Some(ConstValue::Number(v)) if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 => v as i64,
                        _ => { i += 1; continue; }
                    }
                } else { value.len().min(i64::MAX as usize) as i64 };
                if first.is_finite() && first.fract() == 0.0 && first >= i64::MIN as f64 && first <= i64::MAX as f64 {
                    let a = bounded_lua_index(first as i64, value.len(), true).min(value.len());
                    let b = bounded_lua_index(last, value.len(), false).min(value.len());
                    result = Some(ConstValue::String(if b >= a { value[a..b].to_vec() } else { Vec::new() }));
                }
            }
        } else if namespace == "string" && function_name == "rep" && (args.len() == 2 || args.len() == 3) {
            let value = eval_constant_range(source, &tokens, args[0].0, args[0].1);
            let count = eval_constant_range(source, &tokens, args[1].0, args[1].1);
            let sep = if args.len() == 3 { eval_constant_range(source, &tokens, args[2].0, args[2].1) } else { Some(ConstValue::String(Vec::new())) };
            if let (Some(ConstValue::String(value)), Some(ConstValue::Number(n)), Some(ConstValue::String(sep))) = (value, count, sep) {
                if n.is_finite() && n.fract() == 0.0 && (0.0..=4096.0).contains(&n) {
                    let n = n as usize;
                    let total = value.len().saturating_mul(n).saturating_add(sep.len().saturating_mul(n.saturating_sub(1)));
                    if total <= 65536 {
                        let mut out = Vec::with_capacity(total);
                        for part in 0..n {
                            if part > 0 { out.extend_from_slice(&sep); }
                            out.extend_from_slice(&value);
                        }
                        result = Some(ConstValue::String(out));
                    }
                }
            }
        } else if namespace == "table" && function_name == "concat" && (args.len() == 1 || args.len() == 2) {
            let (table_start, table_end) = args[0];
            if table_end > table_start + 1 && tokens[table_start].text == "{" && tokens[table_end - 1].text == "}" {
                let entries = split_arguments(&tokens, table_start + 1, table_end - 1);
                let separator = if args.len() == 2 {
                    match eval_constant_range(source, &tokens, args[1].0, args[1].1) {
                        Some(ConstValue::String(s)) => s,
                        _ => { i += 1; continue; }
                    }
                } else { Vec::new() };
                let mut combined = Vec::new();
                let mut valid = true;
                for (entry_index, (a, b)) in entries.iter().enumerate() {
                    match eval_constant_range(source, &tokens, *a, *b).and_then(|v| concat_bytes(&v)) {
                        Some(value) => {
                            if entry_index > 0 { combined.extend_from_slice(&separator); }
                            combined.extend_from_slice(&value);
                        }
                        None => { valid = false; break; }
                    }
                }
                if valid && combined.len() <= 65536 { result = Some(ConstValue::String(combined)); }
            }
        }

        if let Some(value) = result {
            edits.push(Edit {
                start: tokens[i].start,
                end: tokens[close].end,
                replacement: value.to_lua(),
            });
            i = close + 1;
        } else {
            i += 1;
        }
    }
    apply_edits(source, edits)
}

fn is_assignment_operator(value: &str) -> bool {
    matches!(value, "=" | "+=" | "-=" | "*=" | "/=" | "//=" | "%=" | "^=" | "..=" | "&=" | "|=" | "<<=" | ">>=")
}

fn is_scope_boundary(value: &str) -> bool {
    matches!(value, "if" | "elseif" | "else" | "while" | "for" | "function" | "do" | "repeat" | "end" | "until")
}

fn propagate_local_constants(source: &str) -> (String, usize) {
    let tokens = lex_source(source);
    let mut known: std::collections::HashMap<String, ConstValue> = std::collections::HashMap::new();
    let mut edits = Vec::new();

    for i in 0..tokens.len() {
        let token = &tokens[i];
        if token.kind == TokenKind::Ident && is_scope_boundary(&token.text) {
            known.clear();
            continue;
        }

        if token.text == "local" && i + 3 < tokens.len()
            && tokens[i + 1].kind == TokenKind::Ident && tokens[i + 2].text == "="
        {
            let name = tokens[i + 1].text.clone();
            known.remove(&name);
            let mut pos = i + 3;
            if let Some(node) = parse_expr_bp(source, &tokens, &mut pos, tokens.len(), 1, 0) {
                let suffix_is_call_or_index = pos < tokens.len()
                    && matches!(tokens[pos].text.as_str(), "(" | "[" | "." | ":");
                if !suffix_is_call_or_index {
                    if let Some(value) = node.value {
                        known.insert(name, value);
                    }
                }
            }
            continue;
        }

        if token.kind != TokenKind::Ident { continue; }
        if i > 0 && matches!(tokens[i - 1].text.as_str(), "." | ":" | "local" | "function") {
            continue;
        }
        let table_key = i + 1 < tokens.len() && tokens[i + 1].text == "="
            && i > 0 && matches!(tokens[i - 1].text.as_str(), "{" | ",");
        if table_key { continue; }

        let is_write = i + 1 < tokens.len() && is_assignment_operator(&tokens[i + 1].text);
        if is_write {
            known.remove(&token.text);
            continue;
        }

        if let Some(value) = known.get(&token.text) {
            edits.push(Edit {
                start: token.start,
                end: token.end,
                replacement: value.to_lua(),
            });
        }
    }
    apply_edits(source, edits)
}

#[derive(Clone, Copy, PartialEq)]
enum BlockKind { End, Until }

#[derive(Clone)]
struct IfBranch {
    condition: Option<(usize, usize)>,
    body_start: usize,
    body_end: usize,
}

fn find_then(tokens: &[Token], start: usize) -> Option<usize> {
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    for i in start..tokens.len() {
        match tokens[i].text.as_str() {
            "(" => paren += 1, ")" => paren -= 1,
            "[" => square += 1, "]" => square -= 1,
            "{" => brace += 1, "}" => brace -= 1,
            "then" if paren == 0 && square == 0 && brace == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_if_branches(source: &str, tokens: &[Token], if_index: usize) -> Option<(usize, Vec<IfBranch>)> {
    let then_index = find_then(tokens, if_index + 1)?;
    let mut branches = vec![IfBranch {
        condition: Some((if_index + 1, then_index)),
        body_start: tokens[then_index].end,
        body_end: 0,
    }];
    let mut current = 0usize;
    let mut stack = vec![BlockKind::End];
    let mut i = then_index + 1;

    while i < tokens.len() {
        if stack.len() == 1 && tokens[i].text == "elseif" {
            branches[current].body_end = tokens[i].start;
            let next_then = find_then(tokens, i + 1)?;
            branches.push(IfBranch {
                condition: Some((i + 1, next_then)),
                body_start: tokens[next_then].end,
                body_end: 0,
            });
            current = branches.len() - 1;
            i = next_then + 1;
            continue;
        }
        if stack.len() == 1 && tokens[i].text == "else" {
            branches[current].body_end = tokens[i].start;
            branches.push(IfBranch {
                condition: None,
                body_start: tokens[i].end,
                body_end: 0,
            });
            current = branches.len() - 1;
            i += 1;
            continue;
        }

        match tokens[i].text.as_str() {
            "if" | "function" | "do" => stack.push(BlockKind::End),
            "repeat" => stack.push(BlockKind::Until),
            "end" => {
                if stack.last() == Some(&BlockKind::End) {
                    stack.pop();
                    if stack.is_empty() {
                        branches[current].body_end = tokens[i].start;
                        return Some((i, branches));
                    }
                }
            }
            "until" => {
                if stack.last() == Some(&BlockKind::Until) { stack.pop(); }
            }
            _ => {}
        }
        i += 1;
    }
    let _ = source;
    None
}

fn simplify_if_at(source: &str, tokens: &[Token], if_index: usize) -> Option<Edit> {
    let (end_index, branches) = parse_if_branches(source, tokens, if_index)?;
    let mut chosen: Option<usize> = None;
    let mut determined = true;
    for (index, branch) in branches.iter().enumerate() {
        match branch.condition {
            Some((start, end)) => match eval_constant_range(source, tokens, start, end) {
                Some(value) if value.truthy() => { chosen = Some(index); break; }
                Some(_) => {}
                None => { determined = false; break; }
            },
            None => { chosen = Some(index); break; }
        }
    }
    if !determined { return None; }
    let replacement = if let Some(index) = chosen {
        let branch = &branches[index];
        if branch.body_start <= branch.body_end {
            source[branch.body_start..branch.body_end].to_string()
        } else { String::new() }
    } else {
        let old = &source[tokens[if_index].start..tokens[end_index].end];
        old.chars().filter(|c| *c == '\n' || *c == '\r').collect()
    };
    Some(Edit {
        start: tokens[if_index].start,
        end: tokens[end_index].end,
        replacement,
    })
}

fn simplify_false_while_at(source: &str, tokens: &[Token], while_index: usize) -> Option<Edit> {
    let mut do_index = None;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    for i in while_index + 1..tokens.len() {
        match tokens[i].text.as_str() {
            "(" => paren += 1, ")" => paren -= 1,
            "[" => square += 1, "]" => square -= 1,
            "{" => brace += 1, "}" => brace -= 1,
            "do" if paren == 0 && square == 0 && brace == 0 => { do_index = Some(i); break; }
            _ => {}
        }
    }
    let do_index = do_index?;
    let condition = eval_constant_range(source, tokens, while_index + 1, do_index)?;
    if condition.truthy() { return None; }

    let mut stack = vec![BlockKind::End];
    for i in do_index + 1..tokens.len() {
        match tokens[i].text.as_str() {
            "if" | "function" | "do" => stack.push(BlockKind::End),
            "repeat" => stack.push(BlockKind::Until),
            "end" => {
                if stack.last() == Some(&BlockKind::End) {
                    stack.pop();
                    if stack.is_empty() {
                        let old = &source[tokens[while_index].start..tokens[i].end];
                        let replacement = old.chars().filter(|c| *c == '\n' || *c == '\r').collect();
                        return Some(Edit { start: tokens[while_index].start, end: tokens[i].end, replacement });
                    }
                }
            }
            "until" => {
                if stack.last() == Some(&BlockKind::Until) { stack.pop(); }
            }
            _ => {}
        }
    }
    None
}

fn simplify_constant_control_flow(source: &str) -> (String, usize) {
    let tokens = lex_source(source);
    let mut edits = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i].text == "if" {
            if let Some(edit) = simplify_if_at(source, &tokens, i) {
                i += 1;
                edits.push(edit);
                continue;
            }
        }
        if tokens[i].text == "while" {
            if let Some(edit) = simplify_false_while_at(source, &tokens, i) {
                i += 1;
                edits.push(edit);
                continue;
            }
        }
        i += 1;
    }
    apply_edits(source, edits)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn decode_hex(hex: &str) -> Vec<u8> {
        let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(clean.len() % 2, 0, "fixture hex must have an even length");
        (0..clean.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).expect("valid fixture hex"))
            .collect()
    }

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
    fn decompiles_real_luau_bytecode_fixture() {
        // Real 200-byte compiled Luau chunk from the MIT-licensed
        // PumbaaDev/luau-decompiler test corpus (p03_unary.luac).
        let bytes = decode_hex(include_str!("../tests/fixtures/p03_unary.luac.hex"));
        assert_eq!(bytes.len(), 200);
        assert_eq!(luau_core::bytecode_version(&bytes), Some(6));
        let source = luau_core::decompile(&bytes).expect("fixture should decompile");
        assert!(!source.trim().is_empty());
        assert!(source.contains("function") || source.contains("return") || source.contains("local"));
    }

    #[test]
    fn bytecode_response_includes_recovered_source() {
        let bytes = decode_hex(include_str!("../tests/fixtures/p03_unary.luac.hex"));
        let response = inspect_bytecode(&bytes);
        assert!(response.contains("\"ok\":true"));
        assert!(response.contains("\"mode\":\"bytecode\""));
        assert!(response.contains("\"output\":\""));
        assert!(!response.contains("header inspection only"));
    }

    #[test]
    fn malformed_bytecode_is_rejected_without_fake_output() {
        let response = inspect_bytecode(&[0xff; 32]);
        assert!(response.contains("\"output\":\"\""));
        assert!(response.contains("could not be decoded") || response.contains("rejected"));
    }

    #[test]
    fn identifies_lua_bytecode_header_but_does_not_claim_decompilation() {
        let mut bytes = b"\x1bLua".to_vec();
        bytes.extend_from_slice(&[0x51, 0, 1, 4, 4, 4, 8, 0]);
        let response = inspect_bytecode(&bytes);
        assert!(response.contains("Lua-family bytecode"));
        assert!(response.contains("\"output\":\"\""));
    }
}
