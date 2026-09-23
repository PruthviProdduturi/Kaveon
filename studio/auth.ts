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
import { hasConfiguredSignInProvider } from "./auth/providerConfig";

const adminEmails = (process.env.AUTH_ADMIN_EMAILS ?? "")
  .split(",")
  .map((e) => e.trim().toLowerCase())
  .filter(Boolean);

const adminUsernames = ["pruthviprodduturi"];
const githubConfigured = Boolean(process.env.GITHUB_ID && process.env.GITHUB_SECRET);
const googleConfigured = Boolean(process.env.GOOGLE_ID && process.env.GOOGLE_SECRET);
const microsoftConfigured = Boolean(process.env.AUTH_MICROSOFT_ENTRA_ID_ID && process.env.AUTH_MICROSOFT_ENTRA_ID_SECRET);
const publicClientEnabled = process.env.KAVEON_ENTRA_PUBLIC_CLIENT === "true";
const entraAdmins = new Set((process.env.AUTH_ENTRA_ADMIN_OBJECT_IDS ?? "")
  .split(",").map((value) => value.trim().toLowerCase()).filter(isEntraObjectId));

async function primaryVerifiedGithubEmail(accessToken?: string): Promise<string | null> {
  if (!accessToken) return null;
  try {
    const response = await fetch("https://api.github.com/user/emails", {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${accessToken}`,
        "X-GitHub-Api-Version": "2022-11-28",
      },
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok) return null;
    const emails = await response.json() as Array<{
      email?: string;
      primary?: boolean;
      verified?: boolean;
    }>;
    return emails.find((entry) => entry.primary && entry.verified)?.email ?? null;
  } catch {
    return null;
  }
}

function roleFor(email?: string | null, username?: string | null): "Admin" | "Viewer" {
  // A local Docker installation is a single-user development environment.
  // Keep it frictionless while preserving the hosted allowlist/RBAC model.
  if (process.env.KAVEON_LOCAL_MODE === "true") return "Admin";
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
    ...(githubConfigured
      ? [GitHub({
          clientId: process.env.GITHUB_ID,
          clientSecret: process.env.GITHUB_SECRET,
          authorization: { params: { scope: "read:user user:email" } },
        })]
      : []),
    ...(googleConfigured
      ? [Google({ clientId: process.env.GOOGLE_ID, clientSecret: process.env.GOOGLE_SECRET })]
      : []),
    ...(!publicClientEnabled && microsoftConfigured
      ? [
          MicrosoftEntraID({
            clientId: process.env.AUTH_MICROSOFT_ENTRA_ID_ID,
            clientSecret: process.env.AUTH_MICROSOFT_ENTRA_ID_SECRET,
            issuer: process.env.AUTH_MICROSOFT_ENTRA_ID_ISSUER || "https://login.microsoftonline.com/common/v2.0",
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
    async signIn({ user, account }) {
      if (account?.provider !== "github" || user.email) return true;
      const email = await primaryVerifiedGithubEmail(account.access_token);
      if (!email) return false;
      user.email = email;
      return true;
    },
    // Attach a Kaveon role to the session token so the app can gate on it.
    jwt({ token, profile, user }) {
      const upstreamExpiresAt = user && "upstreamExpiresAt" in user ? user.upstreamExpiresAt : undefined;
      if (typeof upstreamExpiresAt === "number") token.upstreamExpiresAt = upstreamExpiresAt;
      if (typeof token.upstreamExpiresAt === "number" && token.upstreamExpiresAt <= Date.now()) return null;
      const identityProfile = profile as { login?: string; email?: string; preferred_username?: string } | undefined;
      const email = (token.email as string | undefined) ?? identityProfile?.email ?? identityProfile?.preferred_username;
      if (email) token.email = email;
      const username = identityProfile?.login ?? identityProfile?.preferred_username ?? (token.name as string | undefined);
      if (profile) token.role = roleFor(email, username);
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
      if (!hasConfiguredSignInProvider() && (process.env.NODE_ENV === "development" || process.env.KAVEON_LOCAL_MODE === "true") && process.env.KAVEON_DEV_USER_EMAIL) {
        return true;
      }
      const isLoggedIn = !!session?.user;
      const isLoginPage = nextUrl.pathname === "/login";
      if (isLoginPage) return true;
      if (!isLoggedIn) {
        // Carry the requested page so the sign-in screen can return to it.
        const login = new URL("/login", nextUrl);
        login.searchParams.set("callbackUrl", nextUrl.pathname + nextUrl.search);
        return Response.redirect(login);
      }
      return true;
    },
  },
});
