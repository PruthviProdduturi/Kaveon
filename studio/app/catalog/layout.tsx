import { CatalogShell } from "./CatalogShell";

export const metadata = { title: "Catalog" };

export default function CatalogLayout({ children }: { children: React.ReactNode }) {
  return <CatalogShell>{children}</CatalogShell>;
}
