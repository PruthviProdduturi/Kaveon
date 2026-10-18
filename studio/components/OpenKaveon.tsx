import Link from "next/link";
import type { CSSProperties, ReactNode } from "react";

/**
 * The way into the application, and what it says while there is no
 * application to enter.
 *
 * During a maintenance window this renders as a disabled control carrying the
 * return date instead of a link. A link that navigates to a sign-in page for a
 * service which cannot sign anyone in spends a click and a page load to reach
 * a dead end; saying "paused until <date>" in the control itself answers the
 * question before it is asked. The notice dialog carries the detail for anyone
 * who wants more than the date.
 *
 * Driven by the same two variables as `ServiceNotice`, so ending the window is
 * one configuration change and every entry point follows. Unset, this is an
 * ordinary link and costs one boolean.
 */
const PAUSED = process.env.NEXT_PUBLIC_KAVEON_SERVICE_NOTICE;
const RETURNS = process.env.NEXT_PUBLIC_KAVEON_SERVICE_NOTICE_RETURNS;

/** True while the hosted demo is deliberately offline. */
export function servicePaused() {
  return Boolean(PAUSED);
}

type OpenKaveonProps = {
  className?: string;
  style?: CSSProperties;
  children?: ReactNode;
  /** Name the return date in the label. Use where the control has room for it. */
  withDate?: boolean;
};

export function OpenKaveon({ className, style, children, withDate }: OpenKaveonProps) {
  if (!PAUSED) {
    return (
      <Link href="/home" className={className} style={style}>
        {children ?? "Open Kaveon"}
      </Link>
    );
  }

  const label = withDate && RETURNS ? `Paused until ${RETURNS}` : "Paused";

  return (
    <span
      className={className}
      role="link"
      aria-disabled="true"
      title={RETURNS ? `The hosted demo is paused until ${RETURNS}.` : "The hosted demo is paused."}
      style={{
        ...style,
        // Keeps whatever shape the call site gave it, drops the colour and the
        // lift so it reads as unavailable rather than merely unvisited.
        cursor: "not-allowed",
        opacity: 0.5,
        boxShadow: "none",
        filter: "saturate(0.2)",
        pointerEvents: "none",
      }}
    >
      {label}
    </span>
  );
}
