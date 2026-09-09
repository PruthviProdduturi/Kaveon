import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
let cachedJwks: { tenantId: string; key: JWTVerifyGetKey } | undefined;

export type EntraPublicConfig = { clientId: string; tenantId: string; issuer: string };
export type EntraIdentity = { id: string; objectId: string; email: string | null; name: string | null; expiresAt: number };

export function isEntraObjectId(value: string | undefined): value is string {
  return typeof value === "string" && UUID.test(value);
}

export function configuredEntraPublicClient(): EntraPublicConfig | null {
  const clientId = process.env.AUTH_MICROSOFT_ENTRA_ID_ID;
  const configuredIssuer = process.env.AUTH_MICROSOFT_ENTRA_ID_ISSUER;
  if (!isEntraObjectId(clientId) || !configuredIssuer) return null;
  try {
    const issuerUrl = new URL(configuredIssuer);
    const parts = issuerUrl.pathname.split("/").filter(Boolean);
    const tenantId = parts.length === 2 && parts[1] === "v2.0" ? parts[0] : undefined;
    if (issuerUrl.protocol !== "https:" || issuerUrl.hostname !== "login.microsoftonline.com" || issuerUrl.port || issuerUrl.username || issuerUrl.password || issuerUrl.search || issuerUrl.hash || !isEntraObjectId(tenantId)) return null;
    return { clientId: clientId.toLowerCase(), tenantId: tenantId.toLowerCase(), issuer: issuerUrl.toString().replace(/\/$/, "") };
  } catch { return null; }
}

export async function verifyEntraAccessTokenWithKey(token: string, config: EntraPublicConfig, key: JWTVerifyGetKey): Promise<EntraIdentity | null> {
  if (!isEntraObjectId(config.clientId) || !isEntraObjectId(config.tenantId) || token.length === 0 || token.length > 16_384) return null;
  try {
    const { payload } = await jwtVerify(token, key, { issuer: config.issuer, audience: config.clientId, algorithms: ["RS256"] });
    const objectId = typeof payload.oid === "string" && isEntraObjectId(payload.oid) ? payload.oid.toLowerCase() : null;
    const tid = typeof payload.tid === "string" ? payload.tid : null;
    const expiresAt = typeof payload.exp === "number" && Number.isSafeInteger(payload.exp) ? payload.exp * 1_000 : null;
    const scopes = typeof payload.scp === "string" ? payload.scp.split(/\s+/) : [];
    if (!objectId || !tid || !expiresAt || expiresAt <= Date.now() || tid.toLowerCase() !== config.tenantId || !scopes.includes("access_as_user")) return null;
    return { id: `${config.tenantId}:${objectId}`, objectId, email: typeof payload.preferred_username === "string" ? payload.preferred_username : null, name: typeof payload.name === "string" ? payload.name : null, expiresAt };
  } catch { return null; }
}

export async function verifyEntraAccessToken(token: string): Promise<EntraIdentity | null> {
  const config = configuredEntraPublicClient();
  if (!config) return null;
  if (!cachedJwks || cachedJwks.tenantId !== config.tenantId) cachedJwks = { tenantId: config.tenantId, key: createRemoteJWKSet(new URL(`https://login.microsoftonline.com/${config.tenantId}/discovery/v2.0/keys`)) };
  return verifyEntraAccessTokenWithKey(token, config, cachedJwks.key);
}
