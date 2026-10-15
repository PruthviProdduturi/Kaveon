"use client";

import type { ReactNode } from "react";
import type { ListPill } from "../../components/ListPageShell";
import s from "./settings.module.css";

// The card header a Settings section shares with the others. Accepts the
// subset of ListPageShell props existing sections already use, so a page can
// move under the shell without rewriting its body.
interface SettingsSectionProps {
  icon: string;
  title: string;
  subtitle: string;
  pills?: ListPill[];
  action?: ReactNode;
  loading?: boolean;
  loadingMessage?: string;
  error?: string | null;
  /** Offered beside a failed read, so recovery does not mean reloading the browser. */
  onRetry?: () => void;
  children?: ReactNode;
}

export function SettingsSection({ icon, title, subtitle, pills, action, loading, loadingMessage, error, onRetry, children }: SettingsSectionProps) {
  return (
    <div className={s.stack}>
      <section className={s.card}>
        <div className={s.cardHead}>
          <div className={s.cardId}>
            <div className={s.mark}><i className={`fas ${icon}`} aria-hidden="true" /></div>
            <div style={{ minWidth: 0 }}>
              <h2 className={s.cardTitle}>{title}</h2>
              <p className={s.cardSub}>{subtitle}</p>
              {pills && pills.length > 0 && (
                <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 8 }}>
                  {pills.map((pill, i) => <span key={i} className={s.status}>{pill.icon && <i className={`fas ${pill.icon}`} aria-hidden="true" />}{pill.label}</span>)}
                </div>
              )}
            </div>
          </div>
          {/* The header action is what a section is for — registering a
              database, adding a source — and it does not depend on the read
              that lists them. Hidden on error, a failed list also took away
              the ability to add the first entry. */}
          {action && <div>{action}</div>}
        </div>
      </section>
      {loading ? <p className={s.note}>{loadingMessage || "Loading…"}</p>
        : error ? (
          <div className={s.locked} role="status">
            <b>This could not be read</b>
            {error}
            {onRetry && (
              <div style={{ marginTop: 12 }}>
                <button type="button" className={s.ghost} onClick={onRetry}>Try again</button>
              </div>
            )}
          </div>
        )
        : children}
    </div>
  );
}
