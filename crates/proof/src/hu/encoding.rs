//! Magyar Ispell uses UTF-8 words but legacy single-byte affix flags, including bytes that
//! are invalid UTF-8. AF aliases in the .dic and continuation classes are numeric. Map only
//! flag bytes to private-use Unicode scalars; never lossy-decode words or merge distinct flags.

pub(super) fn normalize_aff(bytes: &[u8]) -> Result<String, String> {
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if lines.iter().any(|line| line.starts_with(b"FLAG ")) {
        return std::str::from_utf8(bytes).map(str::to_owned).map_err(|e| format!("Hungarian .aff: {e}"));
    }
    let flags =
        |raw: &[u8]| -> String { raw.iter().filter_map(|&b| char::from_u32(if b < 128 { u32::from(b) } else { 0xe000 + u32::from(b) })).collect() };
    let mut result = String::from("FLAG UTF-8\n");
    for (n, line) in lines.iter().enumerate() {
        let fields: Vec<&[u8]> = line.split(u8::is_ascii_whitespace).filter(|f| !f.is_empty()).collect();
        let Some(&directive) = fields.first() else { continue };
        // Non-semantic legacy-encoded metadata remains intact in the original bundled file.
        if directive.starts_with(b"#") || matches!(directive, b"NAME" | b"HOME" | b"VERSION") {
            continue;
        }
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                result.push(' ');
            }
            if i == 1 && (matches!(directive, b"PFX" | b"SFX") || (directive == b"AF" && !field.iter().all(u8::is_ascii_digit))) {
                result.push_str(&flags(field));
            } else {
                result.push_str(std::str::from_utf8(field).map_err(|e| format!("Hungarian .aff line {}: {e}", n + 1))?);
            }
        }
        result.push('\n');
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_flags_remain_distinct_even_when_they_form_valid_utf8() {
        let aff = b"SET UTF-8\nAF 2\nAF \xc3\xa1\nAF \xe1\nSFX \xe1 Y 1\nSFX \xe1 0 k .\n";
        let text = normalize_aff(aff).unwrap();
        assert!(text.contains("AF \u{e0c3}\u{e0a1}\n"));
        assert!(text.contains("SFX \u{e0e1} Y 1\n"));
        assert!(text.contains("SFX \u{e0e1} 0 k .\n"));
        assert!(normalize_aff(b"SET UTF-8\nTRY \xff\n").is_err());
    }
}
