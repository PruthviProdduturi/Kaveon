// Static generation + caching for the about/landing page.
// No dynamic data — safe to cache aggressively.
export const dynamic = "force-static";
export const revalidate = 3600;

export default function AboutLayout({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
