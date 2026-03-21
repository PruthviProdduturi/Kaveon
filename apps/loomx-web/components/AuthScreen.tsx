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
            <p className="auth-welcome-heading">Welcome back</p>
            <p className="auth-welcome-sub">Sign in to {APP_DISPLAY_NAME} to continue</p>

            {/* Login form */}
            <div className="auth-form">
              <form onSubmit={handleSubmit}>
                <div className="auth-form-fields">
                  {/* Username */}
                  <div className="auth-input-group">
                    <label className="auth-input-label">Username</label>
                    <div className="auth-input-wrapper">
                      <input
                        type="text"
                        className="auth-input"
                        value={username}
                        onChange={e => setUsername(e.target.value)}
                        placeholder="Enter your username"
                        autoComplete="username"
                        autoFocus
                        disabled={isConnecting}
                      />
                      <i className="fas fa-user auth-input-icon" />
                    </div>
                  </div>

                  {/* Password */}
                  <div className="auth-input-group">
                    <label className="auth-input-label">Password</label>
                    <div className="auth-input-wrapper">
                      <input
                        type="password"
                        className="auth-input"
                        value={password}
                        onChange={e => setPassword(e.target.value)}
                        placeholder="Enter your password"
                        autoComplete="current-password"
                        disabled={isConnecting}
                      />
                      <i className="fas fa-lock auth-input-icon" />
                    </div>
                  </div>
                </div>

                <button
                  type="submit"
                  className="auth-button"
                  disabled={isConnecting || !username.trim() || !password}
                >
                  {isConnecting ? (
                    <><i className="fas fa-spinner fa-spin" /><span>Signing In…</span></>
                  ) : (
                    <><i className="fas fa-arrow-right-to-bracket" /><span>Sign In</span></>
                  )}
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

            {/* Default credentials hint */}
            <div className="auth-default-hint">
              <i className="fas fa-circle-info" />
              <span>First-run default: <strong>admin</strong> / <strong>admin</strong></span>
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
