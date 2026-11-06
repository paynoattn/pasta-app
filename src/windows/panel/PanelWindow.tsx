import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import {
  appIcon,
  EVENTS,
  hidePanel,
  historyDelete,
  historyList,
  historyPaste,
  type HistoryFilterView,
  type HistoryItemView,
  type PanelContext,
} from "../../ipc/commands";
import { appHue, hasFormatting, hostname, linkTitle, sanitizeHtml } from "../../lib/preview";
import { countdown, formatBytes, timeAgo } from "../../lib/format";
import { useEvent, useNow, useResource } from "../../state/useResource";
import "./panel.css";

const FILTERS: { value: HistoryFilterView; label: string }[] = [
  { value: "all", label: "All" },
  { value: "text", label: "Text" },
  { value: "links", label: "Links" },
  { value: "code", label: "Code" },
  { value: "from_other_devices", label: "From other devices" },
];

/** Most cards loaded at once. */
const LIMIT = 200;

/** The macOS clipboard panel (#38): a strip of cards along the bottom of the screen. */
export default function PanelWindow() {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<HistoryFilterView>("all");
  const [selected, setSelected] = useState(0);
  const [context, setContext] = useState<PanelContext>({ frontApp: null, canPaste: true });
  const [error, setError] = useState<string | null>(null);
  const search = useRef<HTMLInputElement>(null);
  const strip = useRef<HTMLDivElement>(null);
  const items = useResource(
    () => historyList(query.trim() || null, filter, LIMIT),
    [EVENTS.history],
    `${query}\u0000${filter}`,
  );
  const list = items.data ?? [];
  const current = Math.min(selected, Math.max(0, list.length - 1));
  const now = useNow(30_000);

  useEvent<PanelContext>(EVENTS.panelOpened, (next) => {
    setContext(next);
    setQuery("");
    setFilter("all");
    setSelected(0);
    setError(null);
    strip.current?.focus();
    items.reload();
  });

  useEffect(() => {
    document
      .getElementById(`card-${current}`)
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [current, list.length]);

  const paste = (index: number, plain: boolean) => {
    const item = list[index];
    if (!item) return;
    historyPaste(item.id, plain).catch((err: unknown) => setError(String(err)));
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const inSearch = event.target === search.current;
    if (event.key === "Escape") {
      event.preventDefault();
      if (inSearch && query) setQuery("");
      else void hidePanel();
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      setSelected(Math.min(current + 1, list.length - 1));
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      setSelected(Math.max(current - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      paste(current, event.shiftKey);
    } else if (event.key === "f" && event.metaKey) {
      event.preventDefault();
      search.current?.focus();
    } else if (
      (event.key === "Backspace" || event.key === "Delete") &&
      !inSearch &&
      list[current]
    ) {
      event.preventDefault();
      historyDelete(list[current].id).catch((err: unknown) => setError(String(err)));
    } else if (!inSearch && event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
      // Typing starts a search, as in Paste.
      search.current?.focus();
    }
  };

  const target = context.frontApp ?? "the app in front";
  return (
    <div className="panel" onKeyDown={onKeyDown}>
      <header className="panel__bar">
        <input
          ref={search}
          className="panel__search"
          type="search"
          placeholder="Search"
          aria-label="Search the clipboard history"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setSelected(0);
          }}
        />
        <div className="panel__filters" role="radiogroup" aria-label="Show">
          {FILTERS.map((option) => (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={filter === option.value}
              className={`chip${filter === option.value ? " chip--on" : ""}`}
              onClick={() => {
                setFilter(option.value);
                setSelected(0);
              }}
            >
              {option.label}
            </button>
          ))}
        </div>
      </header>
      <div
        className="panel__strip"
        ref={strip}
        tabIndex={0}
        role="listbox"
        aria-label="Clipboard history"
        aria-activedescendant={list.length ? `card-${current}` : undefined}
      >
        {list.length === 0 && (
          <p className="panel__empty">
            {items.loading
              ? "Loading…"
              : query || filter !== "all"
                ? "Nothing matches."
                : "Copy something and it shows up here."}
          </p>
        )}
        {list.map((item, index) => (
          <Card
            key={item.id}
            id={`card-${index}`}
            item={item}
            now={now}
            selected={index === current}
            onSelect={() => setSelected(index)}
            onPaste={() => paste(index, false)}
          />
        ))}
      </div>
      <footer className="panel__hints">
        {error ? (
          <span className="panel__error" role="alert">
            {error}
          </span>
        ) : (
          <>
            <span>
              <kbd>↩</kbd>{" "}
              {context.canPaste
                ? `Paste into ${target}`
                : "Copy (allow Accessibility to paste directly)"}
            </span>
            <span>
              <kbd>⇧↩</kbd> {context.canPaste ? "Paste as plain text" : "Copy as plain text"}
            </span>
            <span>
              <kbd>⌘F</kbd> Search
            </span>
            <span>
              <kbd>⌫</kbd> Delete
            </span>
            <span>
              <kbd>esc</kbd> Close
            </span>
          </>
        )}
      </footer>
    </div>
  );
}

const icons = new Map<string, Promise<string | null>>();

function useAppIcon(bundleId: string | null): string | null {
  const [icon, setIcon] = useState<{ id: string; url: string | null } | null>(null);
  useEffect(() => {
    if (!bundleId) return;
    let alive = true;
    let pending = icons.get(bundleId);
    if (!pending) {
      pending = appIcon(bundleId).catch(() => null);
      icons.set(bundleId, pending);
    }
    pending.then((url) => alive && setIcon({ id: bundleId, url }));
    return () => {
      alive = false;
    };
  }, [bundleId]);
  return icon && icon.id === bundleId ? icon.url : null;
}

function Card({
  id,
  item,
  now,
  selected,
  onSelect,
  onPaste,
}: {
  id: string;
  item: HistoryItemView;
  now: number;
  selected: boolean;
  onSelect: () => void;
  onPaste: () => void;
}) {
  const icon = useAppIcon(item.sourceAppId);
  const hue = appHue(item.sourceAppId ?? item.sourceApp);
  const app =
    item.sourceApp ?? (item.received ? (item.originDevice ?? "Another device") : "Unknown app");
  return (
    <div
      id={id}
      role="option"
      aria-selected={selected}
      className={`card card--${item.kind}${selected ? " card--selected" : ""}`}
      onClick={onSelect}
      onDoubleClick={onPaste}
    >
      <div className="card__header" style={{ background: `hsl(${hue} 55% 42%)` }}>
        <span className="card__app">{app}</span>
        <span className="card__time">{timeAgo(item.createdAt, now)}</span>
        {icon && <img className="card__icon" src={icon} alt="" width={28} height={28} />}
      </div>
      <div className="card__body">
        <CardBody item={item} />
      </div>
      <div className="card__footer">
        <span>{item.kind === "link" ? "Link" : formatBytes(item.bytes)}</span>
        <span>
          {item.originDevice ?? "This Mac"}
          {item.expiresAt !== null && <> · {countdown(item.expiresAt, now)}</>}
        </span>
      </div>
    </div>
  );
}

/** The preview: a link card, a monospace block, sanitised rich text or plain text (#40). */
function CardBody({ item }: { item: HistoryItemView }) {
  if (item.kind === "link") {
    const title = linkTitle(item.html);
    return (
      <div className="card__link">
        <div className="card__link-host">{hostname(item.preview) ?? item.preview}</div>
        {title && <div className="card__link-title">{title}</div>}
        <div className="card__link-url">{item.preview}</div>
      </div>
    );
  }
  if (item.kind === "code") {
    return <pre className="card__code">{item.preview}</pre>;
  }
  if (item.html) {
    const clean = sanitizeHtml(item.html);
    if (hasFormatting(clean)) {
      return <div className="card__rich" dangerouslySetInnerHTML={{ __html: clean }} />;
    }
  }
  return <div className="card__text">{item.preview}</div>;
}
