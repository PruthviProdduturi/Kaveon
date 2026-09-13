"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useEffect } from "react";

// SQL Lab used to be mounted here as the Catalog's query mode. It is its own
// page again; this route forwards there and keeps every saved link working.
export default function CatalogQueryRedirect() {
  const router = useRouter();
  const search = useSearchParams();
  useEffect(() => {
    const qs = search.toString();
    router.replace(qs ? `/lab?${qs}` : "/lab");
  }, [router, search]);
  return null;
}
