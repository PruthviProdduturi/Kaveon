"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

export default function DatasetsRedirect() {
  const router = useRouter();
  useEffect(() => { router.replace("/workspace?tab=datasets"); }, [router]);
  return null;
}
