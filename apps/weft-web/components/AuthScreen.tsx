import { APP_DISPLAY_NAME, APP_LOGO_URL } from "../constants/branding";
import { WeftLogo } from "./WeftLogo";
import { useAuth } from "../auth/useAuth";

/**
 * AuthScreen — the Weft sign-in surface.
 *
 * Sign-in is delegated to Kaveon Identity (the suite gateway): one account works
 * across Forge, Weft & Anima. Only real identity providers — Microsoft (work,
 * school and personal) and Google. No local passwords, no dev logins.
 */
export function AuthScreen() {
  const { login, isConnecting, error: authError } = useAuth();

  const Logo = (
    <div className="auth-logo">
      {APP_LOGO_URL ? (
        <img src={APP_LOGO_URL} alt={`${APP_DISPLAY_NAME} logo`} style={{ height: 80 }} />
      ) : (
        <WeftLogo size={52} animate="pulse" />
      )}
    </div>
  );

  // Resolving the suite session.
  if (isConnecting) {
    return (
      <div className="auth-section">
        <div className="auth-container">
          {Logo}
          <div className="auth-content">
            <p className="auth-subtitle" style={{ textAlign: "center" }}>
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
              Connecting…
            </p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="auth-section">
      <div className="auth-container">
        {Logo}

        <div className="auth-content">
          <p className="auth-tagline">See the pattern.</p>
          <p className="auth-subtitle">The analyze layer of the Kaveon data platform</p>

          <div className="auth-form" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <button
              className="auth-button"
              onClick={() => void login("microsoft")}
              disabled={isConnecting}
            >
              <i className="fab fa-microsoft" />
              <span>Continue with Microsoft</span>
            </button>

            <button
              className="auth-button"
              onClick={() => void login("google")}
              disabled={isConnecting}
            >
              <i className="fab fa-google" />
              <span>Continue with Google</span>
            </button>
          </div>

          {authError && (
            <div className="auth-error">
              <i className="fas fa-exclamation-triangle" />
              <span>{authError}</span>
            </div>
          )}

          <div className="auth-footer">
            <p>
              One <strong>Kaveon</strong> account works across Forge, Weft &amp; Anima.
              Microsoft covers work, school &amp; personal.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
