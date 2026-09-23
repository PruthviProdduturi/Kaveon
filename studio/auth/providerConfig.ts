/** Whether a real provider is available so local auth can disable the dev bypass. */
export function hasConfiguredSignInProvider(env: NodeJS.ProcessEnv = process.env): boolean {
	return Boolean(
		(env.GITHUB_ID && env.GITHUB_SECRET) ||
		(env.GOOGLE_ID && env.GOOGLE_SECRET) ||
		(env.AUTH_MICROSOFT_ENTRA_ID_ID && env.AUTH_MICROSOFT_ENTRA_ID_SECRET) ||
		(env.KAVEON_ENTRA_PUBLIC_CLIENT === "true" && env.AUTH_MICROSOFT_ENTRA_ID_ID && env.AUTH_MICROSOFT_ENTRA_ID_ISSUER),
	);
}
