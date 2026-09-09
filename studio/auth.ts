/**
 * NextAuth (Auth.js v5) — the independent sign-in for Kaveon.
 *
 * Same model as Forge's portal: OAuth only, no local username/password. Each
 * provider lights up automatically when its client id/secret are present in the
 * environment. Everything runs inside the Next.js app — no external gateway — so
 * Kaveon clones-and-runs and deploys standalone (Vercel / Container Apps).
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
import Credentials from "next-auth/providers/credentials";
import { isEntraObjectId, verifyEntraAccessToken } from "./auth/verifyEntra";
import { sessionConfig } from "./auth/sessionConfig";

const adminEmails = (process.env.AUTH_ADMIN_EMAILS ?? "")
  .split(",")
  .map((e) => e.trim().toLowerCase())
  .filter(Boolean);

const adminUsernames = ["pruthviprodduturi"];
const publicClientEnabled = process.env.KAVEON_ENTRA_PUBLIC_CLIENT === "true";
const entraAdmins = new Set((process.env.AUTH_ENTRA_ADMIN_OBJECT_IDS ?? "")
  .split(",").map((value) => value.trim().toLowerCase()).filter(isEntraObjectId));

function roleFor(email?: string | null, username?: string | null): "Admin" | "Viewer" {
  if (email && adminEmails.includes(email.toLowerCase())) return "Admin";
  if (username && adminUsernames.includes(username.toLowerCase())) return "Admin";
  return "Viewer";
}

export const { handlers, signIn, signOut, auth } = NextAuth({
  providers: [
    ...(publicClientEnabled ? [Credentials({
      id: "entra-public",
      name: "Microsoft Entra ID",
      credentials: { token: { label: "token", type: "password" } },
      async authorize(credentials) {
        const token = typeof credentials?.token === "string" ? credentials.token : "";
        const identity = await verifyEntraAccessToken(token);
        if (!identity) return null;
        return {
          id: identity.id,
          email: identity.email,
          name: identity.name,
          role: entraAdmins.has(identity.objectId) ? "Admin" : "Viewer",
          upstreamExpiresAt: identity.expiresAt,
        };
      },
    })] : []),
    ...(process.env.GITHUB_ID
      ? [GitHub({ clientId: process.env.GITHUB_ID, clientSecret: process.env.GITHUB_SECRET })]
      : []),
    ...(process.env.GOOGLE_ID
      ? [Google({ clientId: process.env.GOOGLE_ID, clientSecret: process.env.GOOGLE_SECRET })]
      : []),
    ...(!publicClientEnabled && process.env.AUTH_MICROSOFT_ENTRA_ID_ID
      ? [
          MicrosoftEntraID({
            clientId: process.env.AUTH_MICROSOFT_ENTRA_ID_ID,
            clientSecret: process.env.AUTH_MICROSOFT_ENTRA_ID_SECRET,
            issuer: process.env.AUTH_MICROSOFT_ENTRA_ID_ISSUER,
            authorization: { params: { prompt: "select_account" } },
          }),
        ]
      : []),
  ],
  pages: {
    signIn: "/login",
  },
  session: sessionConfig(publicClientEnabled),
  callbacks: {
    // Attach a Kaveon role to the session token so the app can gate on it.
    jwt({ token, profile, user }) {
      const upstreamExpiresAt = user && "upstreamExpiresAt" in user ? user.upstreamExpiresAt : undefined;
      if (typeof upstreamExpiresAt === "number") token.upstreamExpiresAt = upstreamExpiresAt;
      if (typeof token.upstreamExpiresAt === "number" && token.upstreamExpiresAt <= Date.now()) return null;
      const username = (profile as { login?: string })?.login ?? (token.name as string | undefined);
      if (profile) token.role = roleFor(token.email as string | undefined, username);
      else if (user && "role" in user) token.role = user.role as string;
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
      // Local-dev bypass — mirrors the API proxy. Container local mode must be
      // opted into explicitly; hosted deployments leave KAVEON_LOCAL_MODE unset.
      if ((process.env.NODE_ENV === "development" || process.env.KAVEON_LOCAL_MODE === "true") && process.env.KAVEON_DEV_USER_EMAIL) {
        return true;
      }
      const isLoggedIn = !!session?.user;
      const isLoginPage = nextUrl.pathname === "/login";
      if (isLoginPage) return true;
      if (!isLoggedIn) return Response.redirect(new URL("/login", nextUrl));
      return true;
    },
  },
});
