"use client";

import { useCallback, useEffect, useState } from "react";

import { msalFetch } from "../utils/msalFetch";

/**
 * One read of a dataset's compiled DLM artifact, shared by everything on the
 * dataset page that needs it.
 *
 * The page used to fetch `/datasets/{id}/dlm` three times for one view — the
 * row-count fact in the header, the Context card, and the context editor's
 * fallback when the curation spec was still empty — and the last two only
 * started after the dataset document had resolved. Against the deployed API
 * that one document costs about 600ms to read (it re-reads the dataset and
 * runs a shadow-read observation of its own), so the page paid for it three
 * times over and put two of those reads in series behind another request.
 *
 * It is one read now, started at mount in parallel with the dataset itself,
 * and handed to each consumer.
 */

export interface DlmManifestColumn {
  name?: string;
  is_dimension?: boolean;
  is_metric?: boolean;
}

export interface DlmManifestMetric {
  name?: string;
  metric_name?: string;
  expression?: string;
}

export interface DlmArtifact {
  status?: string;
  built_at?: string;
  values_indexed?: number;
  manifest?: {
    columns?: DlmManifestColumn[];
    metrics?: DlmManifestMetric[];
  };
  stats_rollup?: {
    generation?: {
      duration_ms?: number;
      built_at?: string;
      answers_precomputed?: number;
      values_indexed?: number;
      rows_scanned?: number;
      scans?: number;
      /** What the source could not deliver, and why. */
      skipped_breakdowns?: { dimension: string; reason: string }[];
    };
    date_range?: { min?: string; max?: string };
    row_counts?: Record<string, number>;
  };
}

/** `loading` until the read settles; `none` when no artifact has been built. */
export type DlmArtifactState = "loading" | "none" | "ready";

export interface DlmArtifactHandle {
  artifact: DlmArtifact | null;
  state: DlmArtifactState;
  /** Row count across the artifact's tables, or null when it reports none. */
  rowCount: number | null;
  reload: () => Promise<void>;
}

export function useDlmArtifact(datasetId: string | undefined, enabled = true): DlmArtifactHandle {
  const [artifact, setArtifact] = useState<DlmArtifact | null>(null);
  const [state, setState] = useState<DlmArtifactState>("loading");

  const reload = useCallback(async () => {
    if (!datasetId || !enabled) return;
    try {
      const res = await msalFetch(`/api/v1/datasets/${datasetId}/dlm`);
      if (res.ok) {
        setArtifact((await res.json()) as DlmArtifact);
        setState("ready");
      } else {
        setArtifact(null);
        setState("none");
      }
    } catch {
      setArtifact(null);
      setState("none");
    }
  }, [datasetId, enabled]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const counts = artifact?.stats_rollup?.row_counts;
  const values = counts ? Object.values(counts).map(Number).filter(Number.isFinite) : [];
  const max = values.length > 0 ? Math.max(...values) : 0;

  return { artifact, state, rowCount: max > 0 ? max : null, reload };
}
