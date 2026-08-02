/**
 * NextAuth (Auth.js v5) — the independent sign-in for Lens.
 *
 * Same model as Forge's portal: OAuth only, no local username/password. Each
 * provider lights up automatically when its client id/secret are present in the
 * environment. Everything runs inside the Next.js app — no external gateway — so
 * Lens clones-and-runs and deploys standalone (Vercel / Container Apps).
 *
 * Providers: GitHub, Google, Microsoft Entra ID (work/school/personal).
 *
 * Required env: AUTH_SECRET (openssl rand -base64 32).
 * Optional per-provider env (see .env.example): GITHUB_ID/GITHUB_SECRET,
 * GOOGLE_ID/GOOGLE_SECRET, AUTH_MICROSOFT_ENTRA_ID_ID/_SECRET/_ISSUER.
 * AUTH_ADMIN_EMAILS (comma-separated) get the Admin role; everyone else Viewer.
 */

import NextAuth from "next-auth";
import GitHub from "next-auth/providers/github";
import Google from "next-auth/providers/google";
import MicrosoftEntraID from "next-auth/providers/microsoft-entra-id";

const adminEmails = (process.env.AUTH_ADMIN_EMAILS ?? "")
  .split(",")
  .map((e) => e.trim().toLowerCase())
  .filter(Boolean);

function roleFor(email?: string | null): "Admin" | "Viewer" {
  return email && adminEmails.includes(email.toLowerCase()) ? "Admin" : "Viewer";
}

export const { handlers, signIn, signOut, auth } = NextAuth({
  providers: [
    ...(process.env.GITHUB_ID
      ? [GitHub({ clientId: process.env.GITHUB_ID, clientSecret: process.env.GITHUB_SECRET })]
      : []),
    ...(process.env.GOOGLE_ID
      ? [Google({ clientId: process.env.GOOGLE_ID, clientSecret: process.env.GOOGLE_SECRET })]
      : []),
    ...(process.env.AUTH_MICROSOFT_ENTRA_ID_ID
      ? [
          MicrosoftEntraID({
            clientId: process.env.AUTH_MICROSOFT_ENTRA_ID_ID,
            clientSecret: process.env.AUTH_MICROSOFT_ENTRA_ID_SECRET,
            issuer: process.env.AUTH_MICROSOFT_ENTRA_ID_ISSUER,
          }),
        ]
      : []),
  ],
  pages: {
    signIn: "/login",
  },
  callbacks: {
    // Attach a Lens role to the session token so the app can gate on it.
    jwt({ token }) {
      token.role = roleFor(token.email as string | undefined);
      return token;
    },
    session({ session, token }) {
      if (session.user) {
        (session.user as { role?: string }).role = (token.role as string) ?? "Viewer";
      }
      return session;
    },
    // Route protection lives in middleware.ts; keep everything else signed-in.
    authorized({ auth: session, request: { nextUrl } }) {
      const isLoggedIn = !!session?.user;
      const isPublic = nextUrl.pathname === "/login";
      if (isPublic) return true;
      if (!isLoggedIn) return Response.redirect(new URL("/login", nextUrl));
      return true;
    },
  },
});
