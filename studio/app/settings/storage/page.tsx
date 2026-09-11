"use client";

import Link from "next/link";
import { CatalogSources } from "../../../components/CatalogSources";
import s from "../settings.module.css";

export default function StoragePage() {
  return (
    <div className={s.stack}>
      <section className={s.card}>
        <div className={s.cardHead}>
          <div className={s.cardId}>
            <div className={s.mark}><i className="fas fa-hard-drive" /></div>
            <div style={{ minWidth: 0 }}>
              <h2 className={s.cardTitle}>KaveonDB catalog sources</h2>
              <p className={s.cardSub}>Storage locations KaveonDB reads in place. Registering one creates the catalog; synchronizing publishes its definition to KaveonDB. Tables are added separately.</p>
            </div>
          </div>
          <Link href="/catalog" className={s.ghost}>Browse the Catalog</Link>
        </div>
      </section>
      <CatalogSources />
    </div>
  );
}
