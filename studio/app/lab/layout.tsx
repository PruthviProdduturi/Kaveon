/**
 * The browser tab's name for this section.
 *
 * The root layout declares `template: "%s — Kaveon"` with a default of
 * "Kaveon", so a section only has to name itself. Only Catalog and Settings
 * did, which left the Library, the Lab, dashboards, datasets and charts all
 * reading as a bare "Kaveon" and indistinguishable once a few tabs were open.
 * Home keeps the default on purpose: it is the front door.
 */
export const metadata = { title: "SQL Lab" };

export default function LabLayout({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
