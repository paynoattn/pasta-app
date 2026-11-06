import DOMPurify from "dompurify";

/**
 * Rich-text previews (#40). Clipboard HTML comes from any app and any web
 * page, so it is sanitised hard before it is rendered: no scripts, styles,
 * forms, frames, images or other remote resources, no event handlers or
 * attributes at all, and links are kept as text only. The CSP
 * (`default-src 'self'`) is a second line of defence.
 */
const ALLOWED_TAGS = [
  "b",
  "strong",
  "i",
  "em",
  "u",
  "s",
  "strike",
  "del",
  "ins",
  "mark",
  "sub",
  "sup",
  "small",
  "p",
  "br",
  "div",
  "span",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "ul",
  "ol",
  "li",
  "blockquote",
  "pre",
  "code",
  "kbd",
  "samp",
  "table",
  "thead",
  "tbody",
  "tr",
  "th",
  "td",
  "hr",
  "a",
];

/** Sanitised HTML that is safe to render as a preview. */
export function sanitizeHtml(html: string): string {
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR: [],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    KEEP_CONTENT: true,
    WHOLE_DOCUMENT: false,
    RETURN_TRUSTED_TYPE: false,
  });
}

/** Whether sanitised HTML still has any formatting worth rendering instead of the plain text. */
export function hasFormatting(sanitized: string): boolean {
  return /<(b|strong|i|em|u|h[1-6]|ul|ol|li|table|pre|code|blockquote|mark)\b/i.test(sanitized);
}

/** `example.com` for a URL, or `null` if it isn't one. */
export function hostname(text: string): string | null {
  const url = text.trim();
  try {
    const parsed = new URL(/^https?:\/\//i.test(url) ? url : `https://${url}`);
    return parsed.hostname.replace(/^www\./, "") || null;
  } catch {
    return null;
  }
}

/** A link card's title: the anchor text of a copied link, if the HTML has one. */
export function linkTitle(html: string | null): string | null {
  if (!html) return null;
  const doc = new DOMParser().parseFromString(sanitizeHtml(html), "text/html");
  const text = doc.querySelector("a")?.textContent?.trim();
  return text && !/^https?:\/\//i.test(text) ? text : null;
}

/** A stable hue for an app, for the card header colour. */
export function appHue(id: string | null): number {
  if (!id) return 210;
  let hash = 0;
  for (const char of id) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  return Math.abs(hash) % 360;
}
