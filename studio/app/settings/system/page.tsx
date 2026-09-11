"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";

// Retired route. Settings now has one section per concern; this URL only exists so old links keep working.
export default function RetiredSettingsRoute() {
  const router = useRouter();
  useEffect(() => { router.replace("/settings/connections"); }, [router]);
  return null;
}
