"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useEffect } from "react";

// SQL Lab is the query mode of the Catalog. This URL forwards there, keeping
// any prefilled query, saved query, or dataset the caller passed along.
export default function LabRedirect() {
  const router = useRouter();
  const search = useSearchParams();
  useEffect(() => {
    const qs = search.toString();
    router.replace(qs ? `/catalog/query?${qs}` : "/catalog/query");
  }, [router, search]);
  return null;
}
