import type { MouseEvent } from "react";
import Link from "next/link";
import { KaveonWordmark } from "./KaveonMark";
import styles from "./PublicHeader.module.css";

type PublicHeaderProps = {
  active?: "about" | "docs";
  /** Optional brand-click handler. The About page uses it to return its own scroll container to the top. */
  onBrandClick?: (event: MouseEvent<HTMLAnchorElement>) => void;
};

export function PublicHeader({ active, onBrandClick }: PublicHeaderProps) {
  const isAbout = active === "about";

  return (
    <header className={`${styles.header} ${styles.aboutHeader} ${isAbout ? "" : styles.docsHeader}`}>
      <Link
        href="/about"
        className={styles.brand}
        aria-label={isAbout ? "Kaveon about, back to top" : "Kaveon home"}
        onClick={onBrandClick}
      >
        <KaveonWordmark height={30} />
      </Link>
      <nav className={`${styles.nav} ${styles.aboutNav}`} aria-label="Kaveon">
        {isAbout ? (
          <a href="#features" className={`${styles.link} ${styles.aboutSectionLink}`}>Features</a>
        ) : (
          <Link href="/about" className={styles.link}>About</Link>
        )}
        <Link href="/docs" className={`${styles.link} ${active === "docs" ? styles.active : ""}`} aria-current={active === "docs" ? "page" : undefined}>Docs</Link>
        <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" className={`${styles.link} ${styles.github}`}>GitHub</a>
        <Link href="/" className={styles.launch}>Launch App</Link>
      </nav>
    </header>
  );
}
