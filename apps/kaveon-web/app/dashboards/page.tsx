"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

export default function DashboardsRedirect() {
  const router = useRouter();
  useEffect(() => { router.replace("/workspace?tab=dashboards"); }, [router]);
  return null;
}
