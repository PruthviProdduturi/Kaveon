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
 * The paused control drops the caller's `className` rather than layering on
 * top of it. Dimming the live button instead -- desaturating its fill and
 * halving its opacity -- produced a grey smudge with barely legible text, and
 * on the header it could not even do that: `.aboutNav .launch` sets its blue
 * with `!important`, which an inline style cannot override. So the disabled
 * state is drawn from scratch as a quiet outline, and only the layout values
 * the call site supplied are carried across.
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
  /**
   * `outline` draws a bordered control, for the places that were buttons.
   * `text` stays inline, for the footer row where a border would be noise.
   */
  paused?: "outline" | "text";
};

/** Layout the call site chose is kept; the paint it chose is dropped. */
const LAYOUT_KEYS = [
  "padding",
  "borderRadius",
  "fontSize",
  "fontWeight",
  "letterSpacing",
  "marginLeft",
] as const;

export function OpenKaveon({
  className,
  style,
  children,
  withDate,
  paused = "outline",
}: OpenKaveonProps) {
  if (!PAUSED) {
    return (
      <Link href="/home" className={className} style={style}>
        {children ?? "Open Kaveon"}
      </Link>
    );
  }

  const label = withDate && RETURNS ? `Paused until ${RETURNS}` : "Paused";
  const title = RETURNS
    ? `The hosted demo is paused until ${RETURNS}.`
    : "The hosted demo is paused.";

  const layout: CSSProperties = {};
  for (const key of LAYOUT_KEYS) {
    const value = style?.[key];
    if (value !== undefined) {
      (layout as Record<string, unknown>)[key] = value;
    }
  }

  const outline = paused === "outline";

  return (
    <span
      role="link"
      aria-disabled="true"
      title={title}
      style={{
        ...layout,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        // Drawn, not dimmed: a transparent ground and a hairline read as
        // unavailable while keeping the label fully legible.
        background: "transparent",
        backgroundImage: "none",
        border: outline ? "1px solid rgba(255,255,255,0.16)" : "none",
        borderRadius: outline ? (layout.borderRadius ?? 10) : 0,
        padding: outline ? (layout.padding ?? "9px 18px") : 0,
        color: "#8b93a0",
        textDecoration: "none",
        boxShadow: "none",
        cursor: "default",
        whiteSpace: "nowrap",
      }}
    >
      {label}
    </span>
  );
}
