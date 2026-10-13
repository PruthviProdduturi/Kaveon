import React from "react";

import styles from "./ChartReadOnly.module.css";

/**
 * The pieces the chart builder's panels are built from when the caller cannot
 * save over the chart.
 *
 * A reader is owed the whole configuration — which dataset, which metrics,
 * which filters — without being offered a control whose result the API would
 * refuse to keep. These components state the configuration instead: same
 * labels, same order, same reading, no inputs.
 */

/** One configuration slot: its label and its current value. */
export const ReadOnlyField: React.FC<{
  label: string;
  /** Shown in place of a value when the chart leaves this slot unset. */
  emptyText?: string;
  children?: React.ReactNode;
}> = ({ label, emptyText = "Not set", children }) => {
  const isEmpty =
    children === null ||
    children === undefined ||
    children === false ||
    (Array.isArray(children) && children.length === 0) ||
    (typeof children === "string" && children.trim() === "");

  return (
    <div className={styles.field}>
      <span className={styles.label}>{label}</span>
      {isEmpty ? (
        <p className={`${styles.value} ${styles.valueEmpty}`}>{emptyText}</p>
      ) : typeof children === "string" || typeof children === "number" ? (
        <p className={styles.value}>{children}</p>
      ) : (
        children
      )}
    </div>
  );
};

/** Several values in one slot — dimensions, metrics, a filter's clauses. */
export const ReadOnlyChips: React.FC<{
  items: string[];
  /** Renders the first item as a logical operator rather than a value. */
  leadingOperator?: string;
}> = ({ items, leadingOperator }) => (
  <div className={styles.chips}>
    {leadingOperator && (
      <span className={`${styles.chip} ${styles.chipOperator}`}>{leadingOperator}</span>
    )}
    {items.map((item, index) => (
      <span key={`${item}-${index}`} className={styles.chip}>
        {item}
      </span>
    ))}
  </div>
);

/** A section rule, matching the editor's own grouping. */
export const ReadOnlySection: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className={styles.section}>
    <span className={styles.sectionLabel}>{children}</span>
    <span className={styles.sectionRule} aria-hidden="true" />
  </div>
);

/**
 * Why this panel reads rather than edits. One per panel, at the top — the
 * fields below it do not each repeat the explanation.
 */
export const ReadOnlyNotice: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className={styles.notice}>
    <i className={`fas fa-lock ${styles.noticeIcon}`} aria-hidden="true" />
    <p className={styles.noticeText}>{children}</p>
  </div>
);

/** The chart's type, stated with its icon the way the picker shows it. */
export const ReadOnlyTypeRow: React.FC<{
  icon: React.ReactNode;
  tint: string;
  name: string;
}> = ({ icon, tint, name }) => (
  <div className={styles.typeRow}>
    <span className={styles.typeIcon} style={{ background: `${tint}15` }}>
      {icon}
    </span>
    <span className={styles.typeName}>{name}</span>
  </div>
);

export default ReadOnlyField;
