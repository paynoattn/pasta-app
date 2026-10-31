//! Windows' `HTML Format` (CF_HTML): a header of byte offsets around the HTML.
//! <https://learn.microsoft.com/windows/win32/dataxchg/html-clipboard-format>

/// Extracts the HTML from a CF_HTML buffer: the whole document when
/// `StartHTML`/`EndHTML` are usable, else the fragment.
pub fn parse(bytes: &[u8]) -> Option<String> {
    let bytes = bytes.split(|&b| b == 0).next().unwrap_or_default();
    let header_end = bytes.iter().position(|&b| b == b'<').unwrap_or(bytes.len());
    let header = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let offset = |key: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(key)?.trim().parse::<i64>().ok())
            .and_then(|v| usize::try_from(v).ok())
    };
    let slice = |start: Option<usize>, end: Option<usize>| {
        let (start, end) = (start?, end?);
        (start < end && end <= bytes.len()).then(|| String::from_utf8_lossy(&bytes[start..end]).into_owned())
    };
    slice(offset("StartHTML:"), offset("EndHTML:"))
        .or_else(|| slice(offset("StartFragment:"), offset("EndFragment:")))
        .filter(|html| !html.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(fragment: &str) -> Vec<u8> {
        let prefix = "<html><body><!--StartFragment-->";
        let suffix = "<!--EndFragment--></body></html>";
        let header_len = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n".len();
        let start_fragment = header_len + prefix.len();
        let end_fragment = start_fragment + fragment.len();
        let end_html = end_fragment + suffix.len();
        format!(
            "Version:0.9\r\nStartHTML:{header_len:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n{prefix}{fragment}{suffix}\0"
        )
        .into_bytes()
    }

    #[test]
    fn extracts_the_document() {
        let html = parse(&wrap("<b>héllo</b>")).unwrap();
        assert_eq!(html, "<html><body><!--StartFragment--><b>héllo</b><!--EndFragment--></body></html>");
    }

    #[test]
    fn falls_back_to_the_fragment() {
        let text = String::from_utf8(wrap("<i>x</i>")).unwrap();
        let start_html = text.lines().find(|line| line.starts_with("StartHTML:")).unwrap().to_owned();
        let no_html = text.replace(&start_html, "StartHTML:-000000001");
        assert_eq!(parse(no_html.as_bytes()).unwrap(), "<i>x</i>");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"Version:0.9\r\nStartHTML:999\r\nEndHTML:5\r\n<b>"), None);
    }
}
