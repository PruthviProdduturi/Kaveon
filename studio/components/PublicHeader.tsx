import Link from "next/link";
import { KaveonWordmark } from "./KaveonMark";
import styles from "./PublicHeader.module.css";

type PublicHeaderProps = { active?: "about" | "docs" };

export function PublicHeader({ active }: PublicHeaderProps) {
  const isAbout = active === "about";

  return (
    <header className={`${styles.header} ${isAbout ? styles.aboutHeader : ""}`}>
      <Link href={isAbout ? "/about" : "/"} className={styles.brand} aria-label={isAbout ? "Kaveon about" : "Kaveon home"}>
        <KaveonWordmark height={isAbout ? 30 : 24} />
      </Link>
      <nav className={`${styles.nav} ${isAbout ? styles.aboutNav : ""}`} aria-label="Kaveon">
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
