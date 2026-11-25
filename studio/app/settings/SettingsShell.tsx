"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useRole } from "../../hooks/useRole";
import s from "./settings.module.css";

const GROUPS = [
  { label: "Connections", icon: "fa-plug", admin: true, pages: [
    { href: "/settings/connections", label: "Overview" },
    { href: "/settings/storage", label: "Lake storage" },
    { href: "/settings/data-sources", label: "External databases" },
  ] },
  { label: "Catalog access", icon: "fa-key", admin: true, pages: [
    { href: "/settings/catalog-access", label: "Catalog access" },
  ] },
  { label: "Operations", icon: "fa-sliders", admin: true, pages: [
    { href: "/settings/governance", label: "Query limits & audit" },
    { href: "/settings/maintenance", label: "Library previews" },
  ] },
  { label: "Preferences", icon: "fa-user-gear", admin: false, pages: [
    { href: "/settings/preferences", label: "Preferences" },
  ] },
];

export function SettingsShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "";
  const { isAdmin, loading } = useRole();
  const current = GROUPS.find(group => group.pages.some(page => pathname === page.href));
  const locked = !!current?.admin && !loading && !isAdmin;

  return (
    <div className={`page-shell ${s.root}`}>
      <header className={s.head}>
        <div>
          <h1 className={s.title}>Settings</h1>
        </div>
      </header>
      <nav className={s.nav} aria-label="Settings sections">
        {GROUPS.filter(group => !group.admin || isAdmin).map(group => (
          <Link key={group.label} href={group.pages[0].href} className={s.navLink}
            aria-current={group === current ? (pathname === group.pages[0].href ? "page" : "location") : undefined}>
            <i className={`fas ${group.icon}`} aria-hidden="true" />{group.label}
          </Link>
        ))}
      </nav>
      {current && current.pages.length > 1 && (!current.admin || isAdmin) && (
        <nav className={s.subnav} aria-label={current.label}>
          {current.pages.map(page => (
            <Link key={page.href} href={page.href} aria-current={pathname === page.href ? "page" : undefined}>
              {page.label}
            </Link>
          ))}
        </nav>
      )}
      {locked ? (
        <div className={s.locked}><b>Administrators only</b>This section requires an administrator account. Your personal settings are available in Preferences.</div>
      ) : children}
    </div>
  );
}
