import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { ButtonHTMLAttributes, HTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, LoaderCircle, X } from "lucide-react";
import { toast } from "sonner";
import { cn } from "../lib/cn";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "secondary" | "ghost" | "outline" | "danger";
  size?: "xs" | "sm" | "md" | "icon" | "icon-sm";
  loading?: boolean;
  loadingLabel?: string;
};

const buttonVariants = {
  default: "border-transparent bg-primary text-primary-foreground shadow-xs hover:bg-primary-hover",
  primary: "border-transparent bg-primary text-primary-foreground shadow-xs hover:bg-primary-hover",
  secondary: "border-transparent bg-secondary text-secondary-foreground hover:bg-secondary-strong",
  ghost: "border-transparent bg-transparent text-muted-strong hover:bg-secondary hover:text-foreground",
  outline: "border-input bg-background text-foreground hover:bg-secondary",
  danger: "border-transparent bg-danger text-primary-foreground hover:bg-danger-hover"
};

const buttonSizes = {
  xs: "h-6 gap-1 px-2 text-xs",
  sm: "h-8 gap-1.5 px-3 text-xs",
  md: "h-9 gap-2 px-4 text-[13px]",
  icon: "h-9 w-9 p-0",
  "icon-sm": "h-8 w-8 p-0"
};

export function Button({ variant = "secondary", size = "md", className, children, loading = false, loadingLabel, disabled, ...props }: ButtonProps) {
  return (
    <button
      className={cn(
        "inline-flex shrink-0 cursor-pointer items-center justify-center whitespace-nowrap rounded-full border font-medium outline-none transition-[background-color,border-color,color,box-shadow,opacity]",
        "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
        "[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:h-4 [&_svg:not([class*='size-'])]:w-4",
        "data-[loading=true]:cursor-wait data-[loading=true]:opacity-90",
        buttonVariants[variant],
        buttonSizes[size],
        className
      )}
      data-loading={loading || undefined}
      disabled={disabled || loading}
      {...props}
    >
      {loading ? <LoaderCircle aria-hidden="true" className="lv-button-spinner" /> : null}
      {loading && loadingLabel ? loadingLabel : children}
    </button>
  );
}

export function IconButton({ className, size = "icon-sm", ...props }: ButtonProps) {
  return <Button size={size} variant="ghost" className={className} {...props} />;
}

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "h-9 min-w-0 rounded-md border border-input bg-input/30 px-3 py-1 text-[13px] text-foreground caret-current shadow-xs outline-none transition-[border-color,box-shadow,color]",
        "placeholder:text-muted focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:cursor-not-allowed disabled:opacity-50",
        className
      )}
      {...props}
    />
  );
}

export function Textarea({ className, ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        "min-h-16 w-full resize-none rounded-md border border-input bg-input/30 px-3 py-2 text-[13px] text-foreground caret-current shadow-xs outline-none transition-[border-color,box-shadow,color]",
        "placeholder:text-muted focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:cursor-not-allowed disabled:opacity-50",
        className
      )}
      {...props}
    />
  );
}

export function Select({ className, children, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <span className="lv-select-shell">
      <select
        className={cn(
          "lv-select h-9 min-w-0 rounded-md border border-input bg-input/30 py-1 pl-3 pr-8 text-[13px] text-foreground shadow-xs outline-none transition-[border-color,box-shadow,color]",
          "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:cursor-not-allowed disabled:opacity-50",
          className
        )}
        {...props}
      >
        {children}
      </select>
      <ChevronDown aria-hidden="true" className="lv-select-chevron" />
    </span>
  );
}

type CheckboxProps = InputHTMLAttributes<HTMLInputElement> & {
  label: string;
};

export function Checkbox({ label, className = "", ...props }: CheckboxProps) {
  return (
    <label className={cn("inline-flex min-h-6 items-center gap-2 text-xs text-foreground", className)}>
      <span className="grid h-4 w-4 place-items-center rounded border border-input bg-input/30 text-primary shadow-xs transition peer-focus-visible:ring-[3px] peer-focus-visible:ring-ring/40">
        <input type="checkbox" aria-label={label} className="peer sr-only" {...props} />
        <Check aria-hidden="true" className="hidden h-3 w-3 peer-checked:block" />
      </span>
      <span className="min-w-0 truncate">{label}</span>
    </label>
  );
}

export function Switch({ label, className, ...props }: CheckboxProps) {
  return (
    <label className={cn("inline-flex min-h-6 items-center gap-2 text-xs text-foreground", className)}>
      <span className="relative inline-flex h-5 w-9 shrink-0 rounded-full border border-input bg-input/30 p-0.5 shadow-xs transition-colors has-[:checked]:bg-primary/70">
        <input type="checkbox" role="switch" aria-label={label} className="peer sr-only" {...props} />
        <span className="h-3.5 w-3.5 rounded-full bg-muted-strong transition-transform peer-checked:translate-x-4 peer-checked:bg-primary-foreground" />
      </span>
      <span className="min-w-0 truncate">{label}</span>
    </label>
  );
}

export function Progress({ value, className = "" }: { value: number; className?: string }) {
  const bounded = Math.max(0, Math.min(100, value));
  return (
    <div className={cn("h-1.5 overflow-hidden rounded-full bg-track", className)} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={bounded}>
      <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${bounded}%` }} />
    </div>
  );
}

export function Field({ label, children, className }: { label: string; children: React.ReactNode; className?: string }) {
  return (
    <label className={cn("grid gap-1.5", className)}>
      <span className="text-xs font-medium text-muted">{label}</span>
      {children}
    </label>
  );
}

export function Panel({ className, children }: { className?: string; children: React.ReactNode }) {
  return <section className={cn("rounded-lg border border-border bg-card text-card-foreground shadow-panel", className)}>{children}</section>;
}

export function SectionHeader({
  icon,
  title,
  description,
  actions,
  className
}: {
  icon?: ReactNode;
  title: string;
  description?: string;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("section-heading", className)}>
      {icon ? <span className="section-icon">{icon}</span> : null}
      <div className="min-w-0">
        <h3>{title}</h3>
        {description ? <p>{description}</p> : null}
      </div>
      {actions ? <div className="ml-auto flex shrink-0 items-center gap-2">{actions}</div> : null}
    </div>
  );
}

export function SidebarItem({
  icon,
  trailing,
  children,
  active = false,
  disabled = false,
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  icon?: ReactNode;
  trailing?: ReactNode;
  active?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={cn("lv-nav-row", active && "active", disabled && "disabled", className)}
      aria-disabled={disabled || undefined}
      {...props}
    >
      {icon}
      <span className="lv-nav-label">{children}</span>
      {trailing ? <span className="lv-nav-trailing">{trailing}</span> : null}
    </button>
  );
}

export function StatusBadge({
  children,
  tone = "neutral",
  dotClassName,
  className
}: {
  children: ReactNode;
  tone?: "neutral" | "primary" | "success" | "danger" | "muted";
  dotClassName?: string;
  className?: string;
}) {
  const toneClass = {
    neutral: "bg-secondary text-muted-strong",
    primary: "bg-primary/15 text-primary",
    success: "bg-success/12 text-success",
    danger: "bg-danger/12 text-danger",
    muted: "bg-secondary text-muted"
  }[tone];

  return (
    <span className={cn("status-pill", toneClass, className)}>
      {dotClassName ? <span className={cn("status-dot", dotClassName)} /> : null}
      {children}
    </span>
  );
}

export function EmptyRow({ title, description, compact = false }: { title: string; description: string; compact?: boolean }) {
  return (
    <div className={cn("lv-table-empty", compact && "compact-empty")}>
      <span>{title}</span>
      <span>{description}</span>
    </div>
  );
}

export function DataTable({ className, children, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("lv-table", className)} {...props}>
      {children}
    </div>
  );
}

export function DataTableHeader({ className, children, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("lv-table-head", className)} {...props}>
      {children}
    </div>
  );
}

export function DataTableRow({ className, children, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("lv-table-row", className)} {...props}>
      {children}
    </div>
  );
}

export function SummaryChip({
  label,
  value,
  dotClassName,
  tone,
  selected = false,
  onClick
}: {
  label: string;
  value: number;
  dotClassName: string;
  tone: "primary" | "success" | "danger";
  selected?: boolean;
  onClick?: () => void;
}) {
  const content = (
    <>
      <span className={cn("summary-dot", dotClassName)} />
      <span className="summary-label">{label}</span>
      <strong>{value}</strong>
    </>
  );
  const className = cn("summary-chip", `summary-chip-${tone}`, value > 0 && "summary-chip-live", onClick && "summary-chip-interactive", selected && "summary-chip-selected");

  if (onClick) {
    return (
      <button type="button" className={className} aria-label={`${label}: ${value}`} aria-pressed={selected} onClick={onClick}>
        {content}
      </button>
    );
  }

  return (
    <div className={className} aria-label={`${label}: ${value}`}>
      {content}
    </div>
  );
}

export function ActivityEventRow({ time, label, dotClassName }: { time: string; label: string; dotClassName: string }) {
  return (
    <li className="activity-row">
      <span className={cn("activity-dot", dotClassName)} />
      <time>{time}</time>
      <span className="line-clamp-2">{label}</span>
    </li>
  );
}

export function Tooltip({ label, children }: { label: string; children: ReactNode }) {
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number; arrowLeft: number; placement: "top" | "bottom" } | null>(null);

  useLayoutEffect(() => {
    if (!visible) return;

    function updatePosition() {
      const trigger = triggerRef.current;
      const tooltip = tooltipRef.current;
      if (!trigger || !tooltip) return;

      const margin = 8;
      const arrowHalf = 4;
      const triggerRect = trigger.getBoundingClientRect();
      const tooltipRect = tooltip.getBoundingClientRect();
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;
      const triggerCenter = triggerRect.left + triggerRect.width / 2;
      const maxLeft = Math.max(margin, viewportWidth - tooltipRect.width - margin);
      const left = Math.min(Math.max(triggerCenter - tooltipRect.width / 2, margin), maxLeft);
      const hasRoomAbove = triggerRect.top >= tooltipRect.height + margin + 6;
      const placement = hasRoomAbove ? "top" : "bottom";
      const rawTop = placement === "top" ? triggerRect.top - tooltipRect.height - margin : triggerRect.bottom + margin;
      const top = Math.min(Math.max(rawTop, margin), Math.max(margin, viewportHeight - tooltipRect.height - margin));
      const arrowLeft = Math.min(Math.max(triggerCenter - left - arrowHalf, 8), Math.max(8, tooltipRect.width - 16));

      setPosition({ left, top, arrowLeft, placement });
    }

    updatePosition();
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [visible, label]);

  return (
    <span
      ref={triggerRef}
      className="relative inline-flex"
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
          setVisible(false);
        }
      }}
      onFocus={() => setVisible(true)}
      onPointerEnter={() => setVisible(true)}
      onPointerLeave={() => setVisible(false)}
    >
      {children}
      {visible
        ? createPortal(
            <span
              ref={tooltipRef}
              role="tooltip"
              className="pointer-events-none fixed z-50 whitespace-nowrap rounded-md bg-foreground px-3 py-1.5 text-xs text-background shadow-panel"
              style={{
                left: position?.left ?? 0,
                top: position?.top ?? 0,
                visibility: position ? "visible" : "hidden"
              }}
            >
              {label}
              <span
                className="absolute h-2 w-2 rotate-45 bg-foreground"
                style={{
                  left: position?.arrowLeft ?? 0,
                  bottom: position?.placement === "top" ? "-4px" : undefined,
                  top: position?.placement === "bottom" ? "-4px" : undefined
                }}
              />
            </span>,
            document.body
          )
        : null}
    </span>
  );
}

export function Popover({
  label,
  trigger,
  open,
  onOpenChange,
  side = "bottom",
  align = "center",
  sideOffset = 8,
  children
}: {
  label: string;
  trigger: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  side?: "top" | "right" | "bottom" | "left";
  align?: "start" | "center" | "end";
  sideOffset?: number;
  children: ReactNode;
}) {
  const triggerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (!open) return;

    function updatePosition() {
      const triggerElement = triggerRef.current;
      const contentElement = contentRef.current;
      if (!triggerElement || !contentElement) return;

      const margin = 8;
      const triggerRect = triggerElement.getBoundingClientRect();
      const contentRect = contentElement.getBoundingClientRect();
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;
      const crossAxisStart = (triggerStart: number, triggerSize: number, contentSize: number) => {
        if (align === "start") return triggerStart;
        if (align === "end") return triggerStart + triggerSize - contentSize;
        return triggerStart + triggerSize / 2 - contentSize / 2;
      };
      const preferredLeft =
        side === "right"
          ? triggerRect.right + sideOffset
          : side === "left"
            ? triggerRect.left - contentRect.width - sideOffset
            : crossAxisStart(triggerRect.left, triggerRect.width, contentRect.width);
      const preferredTop =
        side === "bottom"
          ? triggerRect.bottom + sideOffset
          : side === "top"
            ? triggerRect.top - contentRect.height - sideOffset
            : crossAxisStart(triggerRect.top, triggerRect.height, contentRect.height);
      const left = Math.min(Math.max(preferredLeft, margin), Math.max(margin, viewportWidth - contentRect.width - margin));
      const top = Math.min(Math.max(preferredTop, margin), Math.max(margin, viewportHeight - contentRect.height - margin));

      setPosition({ left, top });
    }

    updatePosition();
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [open, children]);

  useEffect(() => {
    if (!open) return;
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onOpenChange(false);
      }
    }
    function handlePointerDown(event: PointerEvent) {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !contentRef.current?.contains(target)) {
        onOpenChange(false);
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("pointerdown", handlePointerDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [onOpenChange, open]);

  return (
    <div ref={triggerRef} className="relative inline-flex">
      {trigger}
      {open
        ? createPortal(
            <div
              ref={contentRef}
              role="dialog"
              aria-label={label}
              data-popover-side={side}
              data-popover-align={align}
              className="fixed z-50 w-72 max-w-[calc(100vw-16px)] rounded-md border border-border bg-popover p-4 text-xs text-popover-foreground shadow-panel outline-none"
              style={{
                left: position?.left ?? 0,
                top: position?.top ?? 0,
                visibility: position ? "visible" : "hidden"
              }}
            >
              {children}
            </div>,
            document.body
          )
        : null}
    </div>
  );
}

export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  className
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  children: ReactNode;
  className?: string;
}) {
  const titleId = useId();
  const descriptionId = useId();
  const closeRef = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    if (!open) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onOpenChange(false);
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previouslyFocused?.focus();
    };
  }, [onOpenChange, open]);

  if (!open) return null;

  return createPortal(
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50 p-4 backdrop-blur-sm" onMouseDown={() => onOpenChange(false)}>
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        className={cn(
          "relative grid max-h-[85vh] w-full max-w-lg gap-4 overflow-y-auto rounded-lg border border-border bg-background p-6 text-foreground shadow-panel",
          className
        )}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          ref={closeRef}
          type="button"
          className="absolute right-4 top-4 grid h-8 w-8 place-items-center rounded-sm text-muted-strong opacity-75 transition hover:bg-secondary hover:opacity-100 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40"
          aria-label={`Close ${title}`}
          onClick={() => onOpenChange(false)}
        >
          <X aria-hidden="true" className="h-4 w-4" />
        </button>
        <div className="pr-10">
          <h2 id={titleId} className="text-xs font-semibold">{title}</h2>
          {description ? <p id={descriptionId} className="mt-1 text-xs text-muted">{description}</p> : null}
        </div>
        {children}
      </section>
    </div>,
    document.body
  );
}

export function guardedToast(title: string, description: string) {
  toast.warning(title, { description });
}
