"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

export default function ChartsRedirect() {
  const router = useRouter();
  useEffect(() => { router.replace("/workspace?tab=charts"); }, [router]);
  return null;
}
