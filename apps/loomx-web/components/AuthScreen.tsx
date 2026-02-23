import { APP_DISPLAY_NAME, APP_LOGO_URL } from "../constants/branding";
import { LoomXLogo } from "./LoomXLogo";
import { useAuth } from "../auth/useAuth";

export function AuthScreen() {
  const { login, isConnecting, error: authError } = useAuth();

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
            <strong>L</strong>ive <strong>O</strong>perational <strong>O</strong>utcomes & <strong>M</strong>etrics e<strong>X</strong>perience
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
