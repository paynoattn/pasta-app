import { useEffect, useId, useRef, type ReactNode } from "react";
import "./controls.css";
import "./shared.css";

type ButtonVariant = "default" | "primary" | "danger" | "plain";

export function Button({
  children,
  onClick,
  variant = "default",
  disabled,
  type = "button",
  title,
  autoFocus,
}: {
  children: ReactNode;
  onClick?: () => void;
  variant?: ButtonVariant;
  disabled?: boolean;
  type?: "button" | "submit";
  title?: string;
  autoFocus?: boolean;
}) {
  return (
    <button
      type={type}
      className={`btn btn--${variant}`}
      onClick={onClick}
      disabled={disabled}
      title={title}
      autoFocus={autoFocus}
    >
      {children}
    </button>
  );
}

/** An on/off switch (macOS switch, Fluent toggle, Breeze checkbox-switch). */
export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Read by screen readers; the row shows the visible label. */
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className={`toggle${checked ? " toggle--on" : ""}`}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className="toggle__thumb" />
    </button>
  );
}

export interface Choice<T extends string> {
  value: T;
  label: string;
}

/** A segmented control: one of a few choices, all visible. */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  label,
  disabled,
  size = "normal",
}: {
  options: readonly Choice<T>[];
  value: T | null;
  onChange: (value: T) => void;
  label: string;
  disabled?: boolean;
  size?: "normal" | "small";
}) {
  return (
    <div role="radiogroup" aria-label={label} className={`segmented segmented--${size}`}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={`segmented__item${option.value === value ? " segmented__item--on" : ""}`}
          disabled={disabled}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** A native drop-down. Values are strings; map them at the call site. */
export function Select({
  value,
  options,
  onChange,
  label,
  disabled,
}: {
  value: string;
  options: readonly Choice<string>[];
  onChange: (value: string) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <select
      className="select"
      aria-label={label}
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

/** A group of rows with a title: a grouped list on macOS, a card stack on Windows, a form on KDE. */
export function Section({
  title,
  description,
  children,
  id,
}: {
  title?: string;
  description?: ReactNode;
  children: ReactNode;
  id?: string;
}) {
  return (
    <section className="section" id={id}>
      {title && <h2 className="section__title">{title}</h2>}
      {description && <p className="section__description muted">{description}</p>}
      <div className="section__body">{children}</div>
    </section>
  );
}

/** A label (and hint) on the left, a control on the right. */
export function Row({
  label,
  hint,
  children,
  id,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children?: ReactNode;
  id?: string;
}) {
  return (
    <div className="row" id={id}>
      <div className="row__text">
        <div className="row__label">{label}</div>
        {hint && <div className="row__hint muted">{hint}</div>}
      </div>
      {children && <div className="row__control">{children}</div>}
    </div>
  );
}

export type Tone = "info" | "warning" | "error" | "success";

/** A message bar (Fluent InfoBar, Breeze inline message, macOS callout). */
export function InfoBar({
  tone = "info",
  title,
  children,
  action,
}: {
  tone?: Tone;
  title?: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className={`infobar infobar--${tone}`} role={tone === "error" ? "alert" : "status"}>
      <span className="infobar__icon" aria-hidden="true">
        {tone === "success" ? "✓" : tone === "info" ? "i" : "!"}
      </span>
      <div className="infobar__text">
        {title && <strong>{title} </strong>}
        {children}
      </div>
      {action && <div className="infobar__action">{action}</div>}
    </div>
  );
}

export function Badge({
  children,
  tone = "info",
}: {
  children: ReactNode;
  tone?: Tone | "neutral";
}) {
  return <span className={`badge badge--${tone}`}>{children}</span>;
}

/** A modal sheet. Esc and the backdrop close it; focus moves into it. */
export function Dialog({
  open,
  title,
  onClose,
  children,
  footer,
  wide,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
}) {
  const titleId = useId();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    ref.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="dialog__backdrop" onMouseDown={onClose}>
      <div
        className={`dialog${wide ? " dialog--wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        ref={ref}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <h2 className="dialog__title" id={titleId}>
          {title}
        </h2>
        <div className="dialog__body">{children}</div>
        {footer && <div className="dialog__footer">{footer}</div>}
      </div>
    </div>
  );
}

/** A small round status light. */
export function StatusDot({ tone }: { tone: "ok" | "warning" | "error" | "offline" }) {
  return <span className={`dot dot--${tone}`} aria-hidden="true" />;
}

/** An error from Rust, shown under the control that caused it. */
export function ErrorText({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p className="error-text" role="alert">
      {error}
    </p>
  );
}
