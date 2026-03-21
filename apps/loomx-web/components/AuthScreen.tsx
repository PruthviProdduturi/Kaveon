import { useState } from "react";
import { APP_DISPLAY_NAME, APP_LOGO_URL } from "../constants/branding";
import { LoomXLogo } from "./LoomXLogo";
import { useAuth } from "../auth/useAuth";

export function AuthScreen() {
  const { login, isConnecting, error: authError, provider } = useAuth();

  // Local auth form state
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  // Loading state — provider is being resolved
  if (provider === null) {
    return (
      <div className="auth-section">
        <div className="auth-container">
          <div className="auth-logo">
            {APP_LOGO_URL ? (
              <img src={APP_LOGO_URL} alt={`${APP_DISPLAY_NAME} logo`} style={{ height: 80 }} />
            ) : (
              <LoomXLogo size={80} animate="pulse" />
            )}
          </div>
          <div className="auth-content">
            <p className="auth-subtitle" style={{ textAlign: "center" }}>
              <i className="fas fa-spinner fa-spin" style={{ marginRight: 8 }} />
              Loading…
            </p>
          </div>
        </div>
      </div>
    );
  }

  // ── Local login form ────────────────────────────────────────────────────────
  if (provider === "local") {
    const handleSubmit = (e: React.FormEvent) => {
      e.preventDefault();
      if (!username.trim() || !password) return;
      void login({ username: username.trim(), password });
    };

    return (
      <div className="auth-section">
        <div className="auth-container">
          {/* Logo */}
          <div className="auth-logo">
            {APP_LOGO_URL ? (
              <img src={APP_LOGO_URL} alt={`${APP_DISPLAY_NAME} logo`} style={{ height: 80 }} />
            ) : (
              <LoomXLogo size={80} animate="pulse" />
            )}
          </div>

          <div className="auth-content">
            {/* Heading */}
            <p className="auth-tagline" style={{ marginBottom: "0.25rem" }}>
              Welcome to LooMX
            </p>
            <p className="auth-subtitle">Sign in to continue</p>

            {/* Login form */}
            <div className="auth-form">
              <form onSubmit={handleSubmit} style={{ display: "flex", flexDirection: "column", gap: "0.75rem", width: "100%" }}>
                <div style={{ display: "flex", flexDirection: "column", gap: "0.4rem", textAlign: "left" }}>
                  <label style={{ fontSize: 12.5, fontWeight: 600, color: "rgba(255,255,255,0.75)", letterSpacing: "0.03em" }}>
                    Username
                  </label>
                  <input
                    type="text"
                    value={username}
                    onChange={e => setUsername(e.target.value)}
                    placeholder="Enter your username"
                    autoComplete="username"
                    autoFocus
                    disabled={isConnecting}
                    style={{
                      padding: "0.65rem 0.9rem",
                      borderRadius: 8,
                      border: "1px solid rgba(255,255,255,0.2)",
                      background: "rgba(255,255,255,0.1)",
                      color: "white",
                      fontSize: 14,
                      outline: "none",
                      width: "100%",
                      boxSizing: "border-box",
                    }}
                  />
                </div>

                <div style={{ display: "flex", flexDirection: "column", gap: "0.4rem", textAlign: "left" }}>
                  <label style={{ fontSize: 12.5, fontWeight: 600, color: "rgba(255,255,255,0.75)", letterSpacing: "0.03em" }}>
                    Password
                  </label>
                  <input
                    type="password"
                    value={password}
                    onChange={e => setPassword(e.target.value)}
                    placeholder="Enter your password"
                    autoComplete="current-password"
                    disabled={isConnecting}
                    style={{
                      padding: "0.65rem 0.9rem",
                      borderRadius: 8,
                      border: "1px solid rgba(255,255,255,0.2)",
                      background: "rgba(255,255,255,0.1)",
                      color: "white",
                      fontSize: 14,
                      outline: "none",
                      width: "100%",
                      boxSizing: "border-box",
                    }}
                  />
                </div>

                <button
                  type="submit"
                  className="auth-button"
                  disabled={isConnecting || !username.trim() || !password}
                  style={{ marginTop: "0.25rem" }}
                >
                  <i className="fas fa-sign-in-alt" />
                  <span>{isConnecting ? "Signing In…" : "Sign In"}</span>
                  {isConnecting && <i className="fas fa-spinner fa-spin" />}
                </button>
              </form>
            </div>

            {/* Error message */}
            {authError && (
              <div className="auth-error">
                <i className="fas fa-exclamation-triangle" />
                <span>{authError}</span>
              </div>
            )}

            {/* Hint */}
            <div className="auth-footer">
              <p style={{ fontSize: 11.5, opacity: 0.6 }}>Default credentials: admin / admin</p>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // ── Google stub ─────────────────────────────────────────────────────────────
  if (provider === "google") {
    return (
      <div className="auth-section">
        <div className="auth-container">
          <div className="auth-logo">
            {APP_LOGO_URL ? (
              <img src={APP_LOGO_URL} alt={`${APP_DISPLAY_NAME} logo`} style={{ height: 80 }} />
            ) : (
              <LoomXLogo size={80} animate="pulse" />
            )}
          </div>

          <div className="auth-content">
            <p className="auth-tagline">
              <strong>L</strong>ive <strong>O</strong>perational <strong>O</strong>utcomes &amp; <strong>M</strong>etrics e<strong>X</strong>perience
            </p>
            <p className="auth-subtitle">Enterprise data exploration and analytics platform</p>

            <div className="auth-form">
              <button
                className="auth-button"
                onClick={() => void login()}
                disabled={isConnecting}
              >
                <i className="fab fa-google" />
                <span>Sign in with Google</span>
              </button>
            </div>

            {authError && (
              <div className="auth-error">
                <i className="fas fa-exclamation-triangle" />
                <span>{authError}</span>
              </div>
            )}

            <div className="auth-footer">
              <p>Secure sign-in with Google</p>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // ── Azure AD (default) ──────────────────────────────────────────────────────
  return (
    <div className="auth-section">
      <div className="auth-container">
        {/* Logo with animation - outside the card */}
        <div className="auth-logo">
          {APP_LOGO_URL ? (
            <img
              src={APP_LOGO_URL}
              alt={`${APP_DISPLAY_NAME} logo`}
              style={{ height: 80 }}
            />
          ) : (
            <LoomXLogo size={80} animate="pulse" />
          )}
        </div>

        <div className="auth-content">
          {/* Tagline */}
          <p className="auth-tagline">
            <strong>L</strong>ive <strong>O</strong>perational <strong>O</strong>utcomes &amp; <strong>M</strong>etrics e<strong>X</strong>perience
          </p>

          {/* Subtitle */}
          <p className="auth-subtitle">
            Enterprise data exploration and analytics platform
          </p>

          {/* Features Grid */}
          <div className="auth-features">
            <div className="feature-item">
              <i className="fas fa-chart-line"></i>
              <span>Real-time Analytics</span>
            </div>
            <div className="feature-item">
              <i className="fas fa-database"></i>
              <span>Multi-source Support</span>
            </div>
            <div className="feature-item">
              <i className="fas fa-bolt"></i>
              <span>Lightning Fast</span>
            </div>
          </div>

          {/* Sign in button */}
          <div className="auth-form">
            <button
              className="auth-button"
              onClick={() => void login()}
              disabled={isConnecting}
            >
              <i className="fas fa-sign-in-alt"></i>
              <span>{isConnecting ? "Signing In..." : "Sign In with Azure AD"}</span>
              {isConnecting && <i className="fas fa-spinner fa-spin"></i>}
            </button>
          </div>

          {/* Error message */}
          {authError && (
            <div className="auth-error">
              <i className="fas fa-exclamation-triangle" />
              <span>{authError}</span>
            </div>
          )}

          {/* Footer */}
          <div className="auth-footer">
            <p>Secure sign-in with Azure Active Directory</p>
          </div>
        </div>
      </div>
    </div>
  );
}
