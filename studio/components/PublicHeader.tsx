import Link from "next/link";
import { KaveonWordmark } from "./KaveonMark";
import styles from "./PublicHeader.module.css";

type PublicHeaderProps = { active?: "about" | "docs" };

export function PublicHeader({ active }: PublicHeaderProps) {
  return <header className={styles.header}>
    <Link href="/" className={styles.brand} aria-label="Kaveon home"><KaveonWordmark height={24} /></Link>
    <nav className={styles.nav} aria-label="Kaveon">
      <Link href="/about" className={`${styles.link} ${active === "about" ? styles.active : ""}`} aria-current={active === "about" ? "page" : undefined}>About</Link>
      <Link href="/docs" className={`${styles.link} ${active === "docs" ? styles.active : ""}`} aria-current={active === "docs" ? "page" : undefined}>Docs</Link>
      <a href="https://github.com/PruthviProdduturi/Kaveon" target="_blank" rel="noopener noreferrer" className={`${styles.link} ${styles.github}`}>GitHub</a>
      <Link href="/" className={styles.launch}>Launch App</Link>
    </nav>
  </header>;
}
