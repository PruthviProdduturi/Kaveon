"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useRole } from "../../hooks/useRole";
import s from "./settings.module.css";

// Sections are either administration (connections, storage, maintenance) or
// personal (preferences). Everyone sees the shell; administration
// sections explain themselves to non-administrators instead of bouncing them.
const SECTIONS = [
  { href: "/settings/connections", label: "Connections", icon: "fa-plug", admin: true },
  { href: "/settings/storage", label: "Storage", icon: "fa-hard-drive", admin: true },
  { href: "/settings/maintenance", label: "Maintenance", icon: "fa-screwdriver-wrench", admin: true },
  { href: "/settings/preferences", label: "Preferences", icon: "fa-user-gear", admin: false },
] as const;

export function SettingsShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "";
  const { isAdmin, loading } = useRole();
  const current = SECTIONS.find(sec => pathname.startsWith(sec.href));
  const locked = !!current?.admin && !loading && !isAdmin;

  return (
    <div className={`page-shell ${s.root}`}>
      <header className={s.head}>
        <div>
          <h1 className={s.title}>Settings</h1>
          <p className={s.lead}>How this Kaveon instance is connected, and what is yours to change.</p>
        </div>
      </header>
      <nav className={s.nav} aria-label="Settings sections">
        {SECTIONS.filter(sec => !sec.admin).length > 0 && SECTIONS.map(sec => {
          if (sec.admin && !loading && !isAdmin) return null;
          return (
            <Link key={sec.href} href={sec.href} className={s.navLink} aria-current={pathname.startsWith(sec.href) ? "page" : undefined}>
              <i className={`fas ${sec.icon}`} aria-hidden="true" />{sec.label}
            </Link>
          );
        })}
        <span className={s.navGap} />
        {current?.admin && <span className={s.navTag} title="Administrators only">Admin</span>}
      </nav>
      {locked ? (
        <div className={s.locked}><b>Administrators only</b>This section changes how Kaveon is connected. Your account can manage AI keys and preferences.</div>
      ) : children}
    </div>
  );
}
