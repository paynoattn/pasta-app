//! RTF to simple HTML, for previews of items that only have an RTF flavor (#40).
//!
//! Keeps the text, paragraphs and bold / italic / underline; drops fonts,
//! colours, pictures and every other destination. The output is plain
//! escaped text with `<b>`, `<i>`, `<u>` and `<br>`, and the UI sanitises it
//! like any other HTML.

/// Destinations whose content is never shown.
const SKIPPED_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "footer",
    "headerl",
    "headerr",
    "footerl",
    "footerr",
    "listtable",
    "listoverridetable",
    "themedata",
    "datastore",
    "latentstyles",
    "xmlnstbl",
    "rsidtbl",
    "generator",
    "filetbl",
    "revtbl",
    "fldinst",
    "mmathPr",
    "nonshppict",
    "bkmkstart",
    "bkmkend",
    "field",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Format {
    bold: bool,
    italic: bool,
    underline: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Group {
    format: Format,
    skip: bool,
    /// Characters to skip after `\u` (`\ucN`).
    uc: usize,
}

/// Converts `rtf` to HTML. Input that isn't RTF comes back as escaped text.
pub fn to_html(rtf: &str) -> String {
    let mut out = Html::default();
    let mut stack: Vec<Group> = Vec::new();
    let mut group = Group { uc: 1, ..Group::default() };
    let mut pending_skip = 0usize;
    let mut chars = rtf.chars().peekable();
    // Set by `\*`: the next control word starts a destination to skip if unknown.
    let mut ignorable = false;
    // The first control word in a group, which names a destination.
    let mut group_start = false;

    while let Some(c) = chars.next() {
        match c {
            '{' => {
                stack.push(group);
                group_start = true;
                ignorable = false;
            }
            '}' => {
                group = stack.pop().unwrap_or_default();
                group_start = false;
                pending_skip = 0;
            }
            '\\' => {
                let Some(&next) = chars.peek() else { break };
                if next.is_ascii_alphabetic() {
                    let mut word = String::new();
                    while let Some(&ch) = chars.peek().filter(|ch| ch.is_ascii_alphabetic()) {
                        word.push(ch);
                        chars.next();
                    }
                    let mut number = String::new();
                    if chars.peek() == Some(&'-') {
                        number.push('-');
                        chars.next();
                    }
                    while let Some(&ch) = chars.peek().filter(|ch| ch.is_ascii_digit()) {
                        number.push(ch);
                        chars.next();
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }
                    let param: Option<i64> = number.parse().ok();
                    let starts_group = std::mem::take(&mut group_start);
                    if (starts_group || ignorable) && (ignorable || SKIPPED_DESTINATIONS.contains(&word.as_str())) {
                        group.skip = true;
                    }
                    ignorable = false;
                    if group.skip {
                        continue;
                    }
                    match word.as_str() {
                        "par" | "line" | "sect" | "page" => out.newline(),
                        "tab" => out.text(group.format, "\t"),
                        "b" => group.format.bold = param != Some(0),
                        "i" => group.format.italic = param != Some(0),
                        "ul" => group.format.underline = param != Some(0),
                        "ulnone" => group.format.underline = false,
                        "plain" => group.format = Format::default(),
                        "uc" => group.uc = param.and_then(|n| usize::try_from(n).ok()).unwrap_or(1),
                        "u" => {
                            if let Some(code) = param {
                                // Negative values are UTF-16 code units written as signed 16-bit numbers.
                                let unit = if code < 0 { code + 65_536 } else { code };
                                if let Some(ch) = u32::try_from(unit).ok().and_then(char::from_u32) {
                                    out.text(group.format, &ch.to_string());
                                }
                                pending_skip = group.uc;
                            }
                        }
                        "emdash" => out.text(group.format, "—"),
                        "endash" => out.text(group.format, "–"),
                        "bullet" => out.text(group.format, "•"),
                        "lquote" => out.text(group.format, "‘"),
                        "rquote" => out.text(group.format, "’"),
                        "ldblquote" => out.text(group.format, "“"),
                        "rdblquote" => out.text(group.format, "”"),
                        _ => {}
                    }
                } else {
                    chars.next();
                    group_start = false;
                    match next {
                        '*' => ignorable = true,
                        '\'' => {
                            let hex: String = chars.by_ref().take(2).collect();
                            if group.skip {
                                continue;
                            }
                            if pending_skip > 0 {
                                pending_skip -= 1;
                                continue;
                            }
                            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                                out.text(group.format, &cp1252(byte).to_string());
                            }
                        }
                        '~' if !group.skip => out.text(group.format, "\u{a0}"),
                        '\\' | '{' | '}' if !group.skip => out.text(group.format, &next.to_string()),
                        '\n' | '\r' if !group.skip => out.newline(),
                        _ => {}
                    }
                }
            }
            '\r' | '\n' => {}
            _ => {
                group_start = false;
                if group.skip {
                    continue;
                }
                if pending_skip > 0 {
                    pending_skip -= 1;
                    continue;
                }
                out.text(group.format, &c.to_string());
            }
        }
    }
    out.finish()
}

#[derive(Default)]
struct Html {
    html: String,
    open: Format,
}

impl Html {
    fn text(&mut self, format: Format, text: &str) {
        if format != self.open {
            self.close();
            if format.bold {
                self.html.push_str("<b>");
            }
            if format.italic {
                self.html.push_str("<i>");
            }
            if format.underline {
                self.html.push_str("<u>");
            }
            self.open = format;
        }
        for ch in text.chars() {
            match ch {
                '&' => self.html.push_str("&amp;"),
                '<' => self.html.push_str("&lt;"),
                '>' => self.html.push_str("&gt;"),
                '"' => self.html.push_str("&quot;"),
                _ => self.html.push(ch),
            }
        }
    }

    fn close(&mut self) {
        if self.open.underline {
            self.html.push_str("</u>");
        }
        if self.open.italic {
            self.html.push_str("</i>");
        }
        if self.open.bold {
            self.html.push_str("</b>");
        }
        self.open = Format::default();
    }

    fn newline(&mut self) {
        self.close();
        self.html.push_str("<br>");
    }

    fn finish(mut self) -> String {
        self.close();
        while self.html.ends_with("<br>") {
            self.html.truncate(self.html.len() - 4);
        }
        self.html
    }
}

/// Windows-1252, the default RTF code page; bytes it leaves undefined map to U+FFFD.
fn cp1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž', '\u{fffd}',
        '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{fffd}', 'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_text_paragraphs_and_basic_formatting() {
        let rtf = r"{\rtf1\ansi\deff0{\fonttbl{\f0 Helvetica;}}{\colortbl;\red0\green0\blue0;}
\f0\fs24 Hello \b bold\b0  and \i italic\i0 .\par
Second line with \ul under\ulnone .}";
        assert_eq!(to_html(rtf), "Hello <b>bold</b> and <i>italic</i>.<br>Second line with <u>under</u>.");
    }

    #[test]
    fn decodes_escapes_and_unicode() {
        let rtf = r"{\rtf1\ansi caf\'e9 \u8364? \{x\} a\~b \emdash}";
        assert_eq!(to_html(rtf), "café € {x} a\u{a0}b —");
    }

    #[test]
    fn drops_ignorable_destinations_and_escapes_html() {
        let rtf = r"{\rtf1{\*\generator Cocoa;}{\*\unknowndest secret}<script>alert(1)</script>}";
        assert_eq!(to_html(rtf), "&lt;script&gt;alert(1)&lt;/script&gt;");
    }

    #[test]
    fn plain_text_comes_back_escaped() {
        assert_eq!(to_html("a < b"), "a &lt; b");
    }
}
