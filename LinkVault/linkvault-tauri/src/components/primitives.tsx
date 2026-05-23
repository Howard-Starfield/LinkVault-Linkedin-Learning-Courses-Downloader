import { useEffect, useId, useRef } from "react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { createPortal } from "react-dom";
import { Check, X } from "lucide-react";
import { toast } from "sonner";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "ghost" | "outline" | "danger";
  size?: "sm" | "md" | "icon";
};

const buttonVariants = {
  primary: "border-transparent bg-primary text-primary-foreground shadow-glow hover:bg-primary-hover",
  secondary: "border-border bg-secondary text-foreground hover:bg-secondary-strong",
  ghost: "border-transparent bg-transparent text-foreground hover:bg-secondary",
  outline: "border-border bg-card text-foreground hover:bg-secondary",
  danger: "border-transparent bg-danger text-white hover:bg-danger-hover"
};

const buttonSizes = {
  sm: "h-8 px-3 text-xs",
  md: "h-9 px-4 text-sm",
  icon: "h-8 w-8 p-0"
};

export function Button({ variant = "secondary", size = "md", className = "", ...props }: ButtonProps) {
  return (
    <button
      className={`inline-flex shrink-0 items-center justify-center gap-2 rounded-md border font-medium transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-50 ${buttonVariants[variant]} ${buttonSizes[size]} ${className}`}
      {...props}
    />
  );
}

export function IconButton({ className = "", ...props }: ButtonProps) {
  return <Button size="icon" variant="ghost" className={className} {...props} />;
}

export function Input({ className = "", ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={`h-9 min-w-0 rounded-md border border-input bg-field px-3 text-sm text-foreground outline-none transition placeholder:text-muted focus:border-ring focus:ring-2 focus:ring-ring/40 ${className}`}
      {...props}
    />
  );
}

export function Textarea({ className = "", ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={`min-h-20 w-full resize-none rounded-md border border-input bg-field px-3 py-2 text-sm text-foreground outline-none transition placeholder:text-muted focus:border-ring focus:ring-2 focus:ring-ring/40 ${className}`}
      {...props}
    />
  );
}

export function Select({ className = "", children, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={`h-9 min-w-0 rounded-md border border-input bg-field px-3 text-sm text-foreground outline-none transition focus:border-ring focus:ring-2 focus:ring-ring/40 ${className}`}
      {...props}
    >
      {children}
    </select>
  );
}

type CheckboxProps = InputHTMLAttributes<HTMLInputElement> & {
  label: string;
};

export function Checkbox({ label, className = "", ...props }: CheckboxProps) {
  return (
    <label className={`inline-flex min-h-6 items-center gap-2 text-sm text-foreground ${className}`}>
      <span className="grid h-4 w-4 place-items-center rounded border border-input bg-field text-primary">
        <input type="checkbox" aria-label={label} className="peer sr-only" {...props} />
        <Check aria-hidden="true" className="hidden h-3 w-3 peer-checked:block" />
      </span>
      <span className="min-w-0 truncate">{label}</span>
    </label>
  );
}

export function Progress({ value, className = "" }: { value: number; className?: string }) {
  const bounded = Math.max(0, Math.min(100, value));
  return (
    <div className={`h-1.5 overflow-hidden rounded-full bg-track ${className}`} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={bounded}>
      <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${bounded}%` }} />
    </div>
  );
}

export function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="grid gap-1.5">
      <span className="text-xs font-medium text-muted">{label}</span>
      {children}
    </label>
  );
}

export function Panel({ className = "", children }: { className?: string; children: React.ReactNode }) {
  return <section className={`rounded-lg border border-border bg-card shadow-panel ${className}`}>{children}</section>;
}

export function Tooltip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <span className="group/tooltip relative inline-flex">
      {children}
      <span
        role="tooltip"
        className="pointer-events-none absolute bottom-full left-1/2 z-50 mb-2 hidden -translate-x-1/2 whitespace-nowrap rounded-md bg-foreground px-3 py-1.5 text-xs text-background shadow-panel group-focus-within/tooltip:block group-hover/tooltip:block"
      >
        {label}
        <span className="absolute left-1/2 top-full h-2 w-2 -translate-x-1/2 -translate-y-1 rotate-45 bg-foreground" />
      </span>
    </span>
  );
}

export function Popover({
  label,
  trigger,
  open,
  onOpenChange,
  children
}: {
  label: string;
  trigger: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
}) {
  const shellRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onOpenChange(false);
      }
    }
    function handlePointerDown(event: PointerEvent) {
      if (shellRef.current && !shellRef.current.contains(event.target as Node)) {
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
    <div ref={shellRef} className="relative inline-flex">
      {trigger}
      {open ? (
        <div
          role="dialog"
          aria-label={label}
          className="absolute bottom-full right-0 z-50 mb-2 w-72 max-w-[calc(100vw-16px)] rounded-md border border-border bg-card p-4 text-sm text-foreground shadow-panel"
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}

export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  children: ReactNode;
}) {
  const titleId = useId();
  const descriptionId = useId();
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const timeout = window.setTimeout(() => closeRef.current?.focus(), 0);
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onOpenChange(false);
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      window.clearTimeout(timeout);
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
        className="relative grid max-h-[85vh] w-full max-w-lg gap-4 overflow-y-auto rounded-lg border border-border bg-card p-6 text-foreground shadow-panel"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          ref={closeRef}
          type="button"
          className="absolute right-4 top-4 grid h-8 w-8 place-items-center rounded-md border border-border bg-secondary text-muted-strong transition hover:bg-secondary-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={`Close ${title}`}
          onClick={() => onOpenChange(false)}
        >
          <X aria-hidden="true" className="h-4 w-4" />
        </button>
        <div className="pr-10">
          <h2 id={titleId} className="text-base font-semibold">{title}</h2>
          {description ? <p id={descriptionId} className="mt-1 text-sm text-muted">{description}</p> : null}
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
