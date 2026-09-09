/** Keep ordinary OAuth's 30-day lifetime separate from short-lived AKS tokens. */
export function sessionConfig(publicClientEnabled: boolean) {
  return {
    strategy: "jwt" as const,
    maxAge: publicClientEnabled ? 60 * 60 : 30 * 24 * 60 * 60,
  };
}
