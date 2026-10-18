import type {
  ButtonHTMLAttributes,
  ChangeEvent,
  ReactNode,
  SelectHTMLAttributes,
} from "react";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "danger" | "ghost";
  loading?: boolean;
  loadingLabel?: string;
  disabledReason?: string;
};

export function Button({
  variant = "secondary",
  loading = false,
  loadingLabel = "Carregando…",
  disabledReason,
  disabled,
  children,
  className = "",
  ...props
}: ButtonProps) {
  const reasonId = props.id ? `${props.id}-disabled-reason` : undefined;
  const describedBy = [props["aria-describedby"], disabledReason ? reasonId : undefined]
    .filter(Boolean)
    .join(" ") || undefined;
  return (
    <span className="settings-button-wrap">
      <button
        {...props}
        type={props.type ?? "button"}
        className={`settings-button ${variant} ${className}`.trim()}
        disabled={disabled || loading || Boolean(disabledReason)}
        title={disabledReason ?? props.title}
        aria-describedby={describedBy}
        aria-busy={loading || undefined}
      >
        {loading && <span className="settings-spinner" aria-hidden="true" />}
        {loading ? loadingLabel : children}
      </button>
      {disabledReason && <span className="settings-disabled-reason" id={reasonId}>{disabledReason}</span>}
    </span>
  );
}

export type SelectProps = SelectHTMLAttributes<HTMLSelectElement> & {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  loading?: boolean;
  loadingLabel?: string;
};

export function Select({
  id,
  label,
  hint,
  error,
  loading = false,
  loadingLabel = "Carregando…",
  children,
  className = "",
  ...props
}: SelectProps) {
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  const describedBy = [props["aria-describedby"], hintId, errorId].filter(Boolean).join(" ") || undefined;
  return (
    <div className="settings-field">
      <label className="settings-field-label" htmlFor={id}>{label}</label>
      {loading ? (
        <>
          <Skeleton label={loadingLabel} className="settings-select-skeleton" />
          <p className="settings-hint settings-loading-hint"><span className="settings-spinner" aria-hidden="true" />{loadingLabel}</p>
        </>
      ) : (
        <select
          {...props}
          id={id}
          className={`settings-select ${className}`.trim()}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy}
        >
          {children}
        </select>
      )}
      {hint && <p className="settings-hint" id={hintId}>{hint}</p>}
      {error && <p className="settings-error-text" id={errorId}>{error}</p>}
    </div>
  );
}

export type RadioCardProps = {
  name: string;
  value: string;
  checked?: boolean;
  onChange?: (event: ChangeEvent<HTMLInputElement>) => void;
  disabled?: boolean;
  disabledReason?: string;
  children: ReactNode;
};

export function RadioCard({
  name,
  value,
  checked,
  onChange,
  disabled = false,
  disabledReason,
  children,
}: RadioCardProps) {
  const reasonId = `settings-radio-${name}-${value}-reason`;
  const isDisabled = disabled || Boolean(disabledReason);
  return (
    <label className={`settings-radio-card${checked ? " is-selected" : ""}${isDisabled ? " is-disabled" : ""}`}>
      <input
        type="radio"
        name={name}
        value={value}
        checked={checked}
        onChange={onChange}
        disabled={isDisabled}
        aria-describedby={disabledReason ? reasonId : undefined}
      />
      <span className="settings-radio-card-content">
        {children}
        {disabledReason && <span className="settings-radio-card-reason" id={reasonId}>{disabledReason}</span>}
      </span>
    </label>
  );
}

export type SegmentedOption<Value extends string> = { value: Value; label: string };
export type SegmentedProps<Value extends string> = {
  name: string;
  legend: string;
  value: Value;
  options: SegmentedOption<Value>[];
  onChange: (value: Value) => void;
  hint?: string;
  error?: string;
  disabled?: boolean;
};

export function Segmented<Value extends string>({
  name,
  legend,
  value,
  options,
  onChange,
  hint,
  error,
  disabled = false,
}: SegmentedProps<Value>) {
  const hintId = hint ? `${name}-hint` : undefined;
  const errorId = error ? `${name}-error` : undefined;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  return (
    <fieldset className="settings-segmented" role="radiogroup" aria-describedby={describedBy}>
      <legend className="settings-field-label">{legend}</legend>
      <div className="settings-segmented-options">
        {options.map((option) => (
          <label className="settings-segmented-option" key={option.value}>
            <input
              type="radio"
              name={name}
              value={option.value}
              checked={value === option.value}
              disabled={disabled}
              aria-invalid={error ? true : undefined}
              aria-describedby={describedBy}
              onChange={() => onChange(option.value)}
            />
            <span>{option.label}</span>
          </label>
        ))}
      </div>
      {hint && <p className="settings-hint" id={hintId}>{hint}</p>}
      {error && <p className="settings-error-text" id={errorId}>{error}</p>}
    </fieldset>
  );
}

export type SwitchProps = {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  id?: string;
  error?: string;
};

export function Switch({ label, checked, onChange, disabled = false, id, error }: SwitchProps) {
  const errorId = error ? `${id ?? "settings-switch"}-error` : undefined;
  return (
    <button
      id={id}
      className={`settings-switch${checked ? " is-on" : ""}`}
      type="button"
      role="switch"
      aria-label={label}
      aria-checked={checked}
      aria-invalid={error ? true : undefined}
      aria-describedby={errorId}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span aria-hidden="true" />
    </button>
  );
}

export function Progress({ label, value }: { label: string; value?: number }) {
  const determined = value !== undefined && Number.isFinite(value);
  const percent = determined ? Math.max(0, Math.min(100, Math.round(value))) : undefined;
  return (
    <div
      className={`settings-progress${determined ? "" : " is-indeterminate"}`}
      role="progressbar"
      aria-label={label}
      aria-valuemin={determined ? 0 : undefined}
      aria-valuemax={determined ? 100 : undefined}
      aria-valuenow={percent}
    >
      <span style={percent === undefined ? undefined : { width: `${percent}%` }} />
    </div>
  );
}

export function LevelMeter({
  label,
  level,
  active = true,
}: {
  label: string;
  level: number;
  active?: boolean;
}) {
  const percent = active && Number.isFinite(level) ? Math.max(0, Math.min(100, Math.round(level * 100))) : 0;
  const tone = percent > 90 ? "danger" : percent > 70 ? "warning" : "success";
  const noSignal = active && percent === 0;
  return (
    <div
      className={`settings-meter ${tone}${active ? " is-active" : ""}`}
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      aria-valuetext={!active ? "Parado" : noSignal ? "Nenhum som detectado — confira o microfone" : `${percent}%`}
      data-state={!active ? "stopped" : noSignal ? "no-signal" : "active"}
    >
      <span style={{ width: `${percent}%` }} />
    </div>
  );
}

export function Callout({
  tone = "info",
  children,
  action,
}: {
  tone?: "info" | "success" | "warning" | "error";
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className={`settings-callout ${tone}`} role={tone === "error" ? "alert" : "status"}>
      <svg viewBox="0 0 16 16" aria-hidden="true">
        {tone === "warning" ? (
          <><path d="M8 1.5l7 12.5H1z" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round"/><path d="M8 6v3.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round"/><circle cx="8" cy="11.8" r=".9" fill="currentColor"/></>
        ) : tone === "success" ? (
          <><circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" strokeWidth="1.4"/><path d="M4.8 8l2.1 2.1 4.2-4.4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/></>
        ) : tone === "error" ? (
          <><circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" strokeWidth="1.4"/><path d="M8 4v4.4" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round"/><circle cx="8" cy="11" r=".8" fill="currentColor"/></>
        ) : (
          <><circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" strokeWidth="1.4"/><path d="M8 7v4.3" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round"/><circle cx="8" cy="4.7" r=".8" fill="currentColor"/></>
        )}
      </svg>
      <div className="settings-callout-content">{children}{action}</div>
    </div>
  );
}

export function Badge({
  tone = "neutral",
  children,
}: {
  tone?: "active" | "ready" | "recommended" | "unavailable" | "neutral";
  children: ReactNode;
}) {
  return <span className={`settings-badge ${tone}`}>{children}</span>;
}

export function KeyCap({ children }: { children: ReactNode }) {
  return <kbd className="settings-keycap">{children}</kbd>;
}

export function Skeleton({
  label,
  className = "",
}: {
  label: string;
  className?: string;
}) {
  return <span className={`settings-skeleton ${className}`.trim()} role="status" aria-label={label} aria-busy="true" />;
}
