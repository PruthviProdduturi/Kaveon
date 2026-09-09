import assert from "node:assert/strict";
import test from "node:test";
import { generateKeyPair, SignJWT } from "jose";
import { verifyEntraAccessTokenWithKey } from "../auth/verifyEntra.ts";

const tenantId = "11111111-2222-3333-4444-555555555555";
const oid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const clientId = "d0ce7c35-cc10-4ae7-b6be-60d002f43059";
const issuer = `https://login.microsoftonline.com/${tenantId}/v2.0`;
const config = { clientId, tenantId, issuer };
const keys = await generateKeyPair("RS256");
const getKey = async () => keys.publicKey;

async function token(claims = {}, options = {}) {
  return new SignJWT({ tid: tenantId, oid, scp: "access_as_user", ...claims })
    .setProtectedHeader({ alg: "RS256" })
    .setIssuer(options.issuer ?? issuer)
    .setAudience(options.audience ?? clientId)
    .setIssuedAt()
    .setExpirationTime(options.expiration ?? "5m")
    .sign(keys.privateKey);
}

test("accepts a signed delegated Engine access token", async () => {
  const identity = await verifyEntraAccessTokenWithKey(await token({ preferred_username: "person@example.test" }), config, getKey);
  assert.equal(identity?.id, `${tenantId}:${oid}`);
  assert.equal(identity?.email, "person@example.test");
  assert.equal(identity?.name, null);
  assert.ok((identity?.expiresAt ?? 0) > Date.now());
});

test("rejects wrong audience, issuer, expired token, scope, and object id", async () => {
  for (const jwt of [
    await token({}, { audience: "00000003-0000-0000-c000-000000000000" }),
    await token({}, { issuer: "https://login.microsoftonline.com/other/v2.0" }),
    await token({}, { expiration: "-1s" }),
    await token({ scp: "openid profile" }),
    await token({ oid: "not-an-object-id" }),
  ]) {
    assert.equal(await verifyEntraAccessTokenWithKey(jwt, config, getKey), null);
  }
});
