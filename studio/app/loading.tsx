/**
 * Shared App Router fallback. Keeping this as a thin top bar means navigation
 * never replaces a working page with a second full-screen spinner.
 */
export default function Loading() {
  return (
    <div className="global-loading" role="status" aria-live="polite" aria-label="Loading">
      <span className="global-loading__track" aria-hidden="true">
        <span className="global-loading__pulse" />
      </span>
      <span className="sr-only">Loading</span>
    </div>
  );
}
