"use client";
import { useEffect } from "react";
import { useRouter } from "next/navigation";

export default function AuthRedirect() {
  const router = useRouter();
  useEffect(() => { router.replace("/settings/system?tab=auth"); }, [router]);
  return null;
}
