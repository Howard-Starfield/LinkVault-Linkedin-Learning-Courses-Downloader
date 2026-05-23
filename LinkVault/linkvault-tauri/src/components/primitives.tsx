import type { ButtonHTMLAttributes, InputHTMLAttributes, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { Check } from "lucide-react";

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
        <input type="checkbox" className="peer sr-only" {...props} />
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

