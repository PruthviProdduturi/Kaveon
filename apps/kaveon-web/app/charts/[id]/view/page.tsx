"use client";

import React, { useEffect } from "react";
import { useRouter, useParams } from "next/navigation";

export const dynamic = 'force-dynamic';
export const dynamicParams = true;

const ChartViewRedirectPage: React.FC = () => {
  const router = useRouter();
  const params = useParams();

  useEffect(() => {
    const chartId = params?.id as string | undefined;
    if (!chartId) return;

    void router.replace(`/charts/${chartId}`);
  }, [router, params]);

  return (
    <div className="page-shell">
      <header className="page-header">
        <h1 className="page-header-title">Loading chart…</h1>
        <p className="page-header-subtitle">Redirecting to the chart editor.</p>
      </header>
    </div>
  );
};

export default ChartViewRedirectPage;
