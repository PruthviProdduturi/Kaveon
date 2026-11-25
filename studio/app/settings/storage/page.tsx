export const dynamic = "force-dynamic";

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
              <p className={s.cardSub}>Register a storage location, then synchronize its catalog definition with KaveonDB. Add its schemas and tables in the Catalog.</p>
            </div>
          </div>
          <Link href="/catalog" className={s.ghost}>Browse the Catalog</Link>
        </div>
      </section>
      <CatalogSources localInstallation={process.env.KAVEON_LOCAL_MODE === "true"} />
    </div>
  );
}
