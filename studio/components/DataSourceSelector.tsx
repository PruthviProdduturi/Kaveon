"use client";

import { useCallback, useEffect, useId, useState } from "react";
import { API_BASE } from "../config";
import { msalFetch } from "../utils/msalFetch";
import { refusalMessage } from "../utils/demoQuota";
import { useAuth } from "../auth/useAuth";
import { UnknownLine, reasonFromStatus, type ReadReason } from "./Reading";

interface DataSource {
  id: number;
  name: string;
  type: string;
  database_name?: string;
  region: string;
  is_active: boolean;
  is_favorite?: number;
  table_count?: number;
}

interface DataSourceSelectorProps {
  value?: number | null;
  onChange: (dataSourceId: number | null) => void;
  label?: string;
  required?: boolean;
  className?: string;
}

/** A read that did not answer, kept apart from a read that answered with nothing. */
interface ReadFailure { reason: ReadReason | null; detail: string }

export function DataSourceSelector({
  value,
  onChange,
  label = "Data Source",
  required = false,
  className = "chart-builder-field"
}: DataSourceSelectorProps) {
  const { account, isAuthenticated } = useAuth();
  const [dataSources, setDataSources] = useState<DataSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [failure, setFailure] = useState<ReadFailure | null>(null);
  const selectId = useId();

  const loadDataSources = useCallback(async () => {
    try {
      setLoading(true);
      const userEmail = account?.email || account?.username || null;
      const response = await msalFetch(`${API_BASE}/api/v1/data-sources/active`, {
        headers: userEmail ? { 'x-user-email': userEmail } : undefined
      });
      const body: unknown = await response.json().catch(() => null);
      if (!response.ok) {
        // "Failed to load data sources" was the message for every status, and
        // the route's own explanation was discarded with the body.
        setFailure({
          reason: reasonFromStatus(response.status),
          detail: refusalMessage(body, `The server answered ${response.status}.`),
        });
        return;
      }
      const listing = (body as { dataSources?: unknown } | null)?.dataSources;
      if (!Array.isArray(listing)) {
        // A body that is not a listing is not an empty listing.
        setFailure({ reason: "invalid_listing", detail: "" });
        return;
      }
      setFailure(null);
      setDataSources(listing as DataSource[]);

      // This used to auto-select the caller's favourite whenever `value` was
      // falsy — and `value` is falsy for the first render of EVERY edit, since
      // the dataset page starts `currentDataSourceId` at null and fills it from
      // the dataset it is loading, in a different effect, with no ordering
      // between the two. So opening an existing dataset for editing could call
      // `onChange(favourite.id)` and re-point it at a data source the operator
      // never chose, before they had seen the one it was bound to. A save then
      // wrote that.
      //
      // Nothing is lost by not doing it here: the datasets page already selects
      // the favourite, or the first source, in its own initial load — and
      // deliberately only for a new dataset. A component that cannot tell a new
      // form from a loading one has no business making that choice for it.
    } catch {
      setFailure({ reason: null, detail: "The Kaveon API could not be reached from this browser." });
    } finally {
      setLoading(false);
    }
  }, [account?.email, account?.username]);

  useEffect(() => {
    if (!isAuthenticated) return;
    void loadDataSources();
  }, [isAuthenticated, loadDataSources]);

  if (loading) {
    return (
      <div className={className}>
        <label className="chart-builder-label" htmlFor={selectId}>
          <span>{label} {required && '*'}</span>
        </label>
        {/* #9ca3af measured 2.43:1 on the light theme and did not follow the
            theme at all; the muted token is the tone for this and is defined
            for both grounds. */}
        <div style={{ padding: '0.5rem', color: 'var(--text-muted)', fontSize: '0.85rem' }}>
          <i className="fas fa-spinner fa-spin" aria-hidden="true" /> Loading data sources...
        </div>
      </div>
    );
  }

  // A read that did not answer is reported as an unknown rather than as an
  // empty list of data sources: this used to render a one-line error that
  // claimed the load had failed whatever the server said, and left no way to
  // ask again short of reloading the page.
  if (failure && dataSources.length === 0) {
    return (
      <div className={className}>
        <label className="chart-builder-label">
          <span>{label} {required && '*'}</span>
        </label>
        <div style={{ padding: '0.5rem', fontSize: '0.85rem' }}>
          <UnknownLine
            what="The registered data sources"
            says="which of them this dataset can read from"
            reason={failure.reason}
            detail={failure.detail || null}
          />
          <button
            type="button"
            onClick={() => void loadDataSources()}
            style={{
              display: 'block', marginTop: 6, padding: 0,
              background: 'none', border: 'none', cursor: 'pointer',
              color: 'var(--accent)', font: 'inherit', textDecoration: 'underline',
            }}
          >
            Try again
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className={className}>
      <label className="chart-builder-label" htmlFor={selectId}>
        <span>{label} {required && '*'}</span>
      </label>
      <select
        id={selectId}
        className="chart-builder-select"
        value={value || ''}
        onChange={(e) => onChange(e.target.value ? parseInt(e.target.value, 10) : null)}
        required={required}
      >
        {!required && <option value="">-- Select Data Source --</option>}
        {dataSources.map((ds) => (
          <option key={ds.id} value={ds.id}>
            {ds.name}
          </option>
        ))}
        {dataSources.length === 0 && (
          <option value="" disabled>No data sources available</option>
        )}
      </select>
      {/* Rows from the last read that answered. Without this the list would
          look current while the registry was not being reached. */}
      {failure && (
        <div style={{ marginTop: 6, fontSize: '0.8rem', color: 'var(--text-muted)' }}>
          These are the data sources from the last reading; the registry is not answering now.
          {' '}
          <button
            type="button"
            onClick={() => void loadDataSources()}
            style={{ padding: 0, background: 'none', border: 'none', cursor: 'pointer', color: 'var(--accent)', font: 'inherit', textDecoration: 'underline' }}
          >
            Try again
          </button>
        </div>
      )}
    </div>
  );
}
