"use client";

import { ReactNode } from "react";
import { useAuth } from "../auth/useAuth";
import { AuthScreen } from "./AuthScreen";
import { Layout } from "./Layout";
import { LoadingOverlay } from "./LoadingOverlay";

interface ClientLayoutProps {
  children: ReactNode;
}

/**
 * Client-side layout wrapper that enforces authentication.
 * This component uses the useAuth hook to check authentication state
 * and renders the appropriate screen based on that state.
 */
export function ClientLayout({ children }: ClientLayoutProps) {
  const { isAuthenticated, isConnecting } = useAuth();

  // Show loading overlay while checking for existing auth session
  if (isConnecting) {
    return <LoadingOverlay />;
  }

  // Redirect to login if no active session
  if (!isAuthenticated) {
    return <AuthScreen />;
  }

  // Render authenticated content within the Layout
  return <Layout>{children}</Layout>;
}
