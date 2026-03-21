"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { API_BASE } from "../../../config";
import { msalFetch } from "../../../utils/msalFetch";
import { useAuth, type UserRole } from "../../../auth/useAuth";
import { useRole } from "../../../hooks/useRole";
import { useTheme } from "../../../contexts/ThemeContext";
import { LoomXLoading } from "../../../components/LoomXLoading";
import { Button } from "../../../components/Button";

interface RoleAssignment {
  user_email: string;
  role: UserRole;
  granted_by: string | null;
  granted_at: string | null;
}

const ROLES: UserRole[] = ["Viewer", "Analyst", "Editor", "Admin"];

const ROLE_META: Record<UserRole, {
  icon: string;
  bg: string;
  color: string;
  border: string;
  gradient: string;
  description: string;
}> = {
  Viewer:  {
    icon: "fa-eye",
    bg: "#f8fafc", color: "#475569", border: "#e2e8f0",
    gradient: "linear-gradient(135deg, #64748b, #94a3b8)",
    description: "View published dashboards and charts. No SQL Lab or creation.",
  },
  Analyst: {
    icon: "fa-chart-line",
    bg: "#eff6ff", color: "#1d4ed8", border: "#bfdbfe",
    gradient: "linear-gradient(135deg, #2563eb, #60a5fa)",
    description: "SQL Lab, build charts and datasets, access internal content.",
  },
  Editor:  {
    icon: "fa-pen-nib",
    bg: "#f5f3ff", color: "#6d28d9", border: "#ddd6fe",
    gradient: "linear-gradient(135deg, #7c3aed, #a78bfa)",
    description: "All Analyst permissions plus ability to publish content.",
  },
  Admin:   {
    icon: "fa-shield-alt",
    bg: "#fff1f2", color: "#be123c", border: "#fecdd3",
    gradient: "linear-gradient(135deg, #e11d48, #fb7185)",
    description: "Full access including user and data source management.",
  },
};

function getInitials(email: string): string {
  const local = email.split("@")[0];
  const parts = local.split(/[._-]/);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return local.slice(0, 2).toUpperCase();
}

function formatDate(val: string | null) {
  if (!val) return "—";
  try {
    return new Date(val).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  } catch { return val; }
}

export default function UsersPage() {
  const router = useRouter();
  const { account, isAuthenticated } = useAuth();
  const { isAdmin } = useRole();
  const { primaryColor, gradientColors } = useTheme();

  const [assignments, setAssignments] = useState<RoleAssignment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showAssignModal, setShowAssignModal] = useState(false);
  const [editingAssignment, setEditingAssignment] = useState<RoleAssignment | null>(null);
  const [removingEmail, setRemovingEmail] = useState<string | null>(null);

  useEffect(() => {
    if (!isAuthenticated) return;
    if (isAdmin === false) { router.replace("/"); return; }
    if (isAdmin) loadAssignments();
  }, [isAuthenticated, isAdmin]);

  async function loadAssignments() {
    try {
      setLoading(true);
      setError(null);
      const res = await msalFetch(`${API_BASE}/api/v1/users`, {
        headers: { "x-user-email": account?.email || account?.username || "" },
      });
      if (!res.ok) throw new Error(`Failed to load users (${res.status})`);
      const data = await res.json();
      setAssignments(Array.isArray(data) ? data : (data.users ?? []));
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load users");
    } finally {
      setLoading(false);
    }
  }

  async function handleRemove(email: string) {
    if (!confirm(`Remove role assignment for ${email}?\n\nThey will fall back to the default Viewer role.`)) return;
    setRemovingEmail(email);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/users/${encodeURIComponent(email)}/role`, {
        method: "DELETE",
        headers: { "x-user-email": account?.email || account?.username || "" },
      });
      if (!res.ok && res.status !== 204) throw new Error(`Failed (${res.status})`);
      await loadAssignments();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to remove role");
    } finally {
      setRemovingEmail(null);
    }
  }

  if (!isAdmin) return null;

  // Role counts for stat cards
  const roleCounts = ROLES.reduce((acc, r) => {
    acc[r] = assignments.filter(a => a.role === r).length;
    return acc;
  }, {} as Record<UserRole, number>);

  return (
    <div className="page-shell animate-fade-in">

      {/* ── Hero header ── */}
      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between",
        flexWrap: "wrap", gap: "1rem",
        marginBottom: "1.5rem",
        padding: "1.25rem 1.5rem",
        background: "white",
        borderRadius: 12,
        border: "1px solid #e5e7eb",
        boxShadow: "0 1px 4px rgba(15,23,42,0.06)",
      }}>
        {/* Left: identity + stats */}
        <div style={{ display: "flex", alignItems: "center", gap: "1.25rem", flexWrap: "wrap", flex: 1, minWidth: 0 }}>
          <div style={{
            width: 48, height: 48, borderRadius: 12, flexShrink: 0,
            background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
            display: "flex", alignItems: "center", justifyContent: "center",
            boxShadow: `0 4px 14px ${primaryColor}35`,
          }}>
            <i className="fas fa-users-cog" style={{ color: "white", fontSize: 20 }} />
          </div>
          <div style={{ minWidth: 0 }}>
            <h1 style={{ margin: 0, fontSize: "1.15rem", fontWeight: 700, color: "#0f172a" }}>User Management</h1>
            <p style={{ margin: "3px 0 0", fontSize: "0.85rem", color: "#64748b", lineHeight: 1.4 }}>
              Assign roles to control access. Azure AD App Roles take precedence over DB assignments.
            </p>
          </div>
          {/* Inline role summary pills */}
          {!loading && !error && (
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap", marginLeft: "0.5rem" }}>
              {ROLES.map(role => {
                const meta = ROLE_META[role];
                const count = roleCounts[role];
                return (
                  <div key={role} style={{
                    display: "flex", alignItems: "center", gap: "0.4rem",
                    padding: "0.3rem 0.75rem", borderRadius: 20,
                    background: meta.bg, border: `1px solid ${meta.border}`,
                    fontSize: 12, fontWeight: 600, color: meta.color,
                    whiteSpace: "nowrap",
                  }}>
                    <i className={`fas ${meta.icon}`} style={{ fontSize: 10 }} />
                    <span>{count} {role}{count !== 1 ? "s" : ""}</span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        {/* Right: action */}
        <Button onClick={() => { setEditingAssignment(null); setShowAssignModal(true); }} style={{ flexShrink: 0 }}>
          <i className="fas fa-user-plus" /> Assign Role
        </Button>
      </div>

      {/* ── Error ── */}
      {error && (
        <div className="card page-empty-card">
          <p className="page-empty-title">Problem loading users</p>
          <p className="page-empty-body">{error}</p>
        </div>
      )}

      {/* ── Loading ── */}
      {isAuthenticated && loading && <LoomXLoading message="Loading user assignments" />}

      {!loading && !error && (
        <>

          {/* ── Empty state ── */}
          {assignments.length === 0 && (
            <div className="card page-empty-card">
              <div style={{
                width: 56, height: 56, borderRadius: 14, margin: "0 auto 12px",
                background: `linear-gradient(135deg, ${gradientColors.light}, ${gradientColors.dark})`,
                display: "flex", alignItems: "center", justifyContent: "center",
              }}>
                <i className="fas fa-users" style={{ color: "white", fontSize: 22 }} />
              </div>
              <p className="page-empty-title">No explicit role assignments</p>
              <p className="page-empty-body">
                All authenticated users default to <strong>Viewer</strong> until assigned a role here
                or via Azure AD Enterprise Applications.
              </p>
              <Button style={{ marginTop: 12 }} onClick={() => setShowAssignModal(true)}>
                <i className="fas fa-user-plus" /> Assign First Role
              </Button>
            </div>
          )}

          {/* ── Assignments table ── */}
          {assignments.length > 0 && (
            <div className="card">
              <div className="results-table-container">
                <table className="results-table">
                  <thead>
                    <tr>
                      <th><span className="column-header-label">User</span></th>
                      <th><span className="column-header-label">Role</span></th>
                      <th><span className="column-header-label">Granted By</span></th>
                      <th><span className="column-header-label">Granted At</span></th>
                      <th><span className="column-header-label">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {assignments.map(a => {
                      const meta = ROLE_META[a.role];
                      return (
                        <tr key={a.user_email}>
                          <td>
                            <div style={{ display: "flex", alignItems: "center", gap: "0.75rem" }}>
                              {/* Avatar */}
                              <div style={{
                                width: 32, height: 32, borderRadius: "50%", flexShrink: 0,
                                background: meta.gradient,
                                display: "flex", alignItems: "center", justifyContent: "center",
                                fontSize: 11, fontWeight: 700, color: "white",
                                letterSpacing: "0.5px",
                              }}>
                                {getInitials(a.user_email)}
                              </div>
                              <span style={{ fontSize: 13, fontFamily: "monospace" }}>{a.user_email}</span>
                            </div>
                          </td>
                          <td>
                            <span style={{
                              display: "inline-flex", alignItems: "center", gap: "0.35rem",
                              padding: "0.25rem 0.65rem", borderRadius: 6,
                              fontSize: "0.75rem", fontWeight: 600,
                              background: meta.bg, color: meta.color,
                              border: `1px solid ${meta.border}`,
                            }}>
                              <i className={`fas ${meta.icon}`} style={{ fontSize: "0.65rem" }} />
                              {a.role}
                            </span>
                          </td>
                          <td className="muted" style={{ fontSize: 13 }}>{a.granted_by ?? "—"}</td>
                          <td className="muted" style={{ fontSize: 13 }}>{formatDate(a.granted_at)}</td>
                          <td className="actions-cell">
                            <div className="row-actions">
                              <button
                                type="button"
                                className="action-icon-btn"
                                title="Edit role"
                                aria-label="Edit role"
                                onClick={() => { setEditingAssignment(a); setShowAssignModal(true); }}
                              >
                                <i className="fas fa-edit" aria-hidden="true" />
                              </button>
                              <button
                                type="button"
                                className="action-icon-btn"
                                title="Remove role assignment"
                                aria-label="Remove role assignment"
                                disabled={removingEmail === a.user_email}
                                onClick={() => handleRemove(a.user_email)}
                              >
                                <i className={removingEmail === a.user_email ? "fas fa-spinner fa-spin" : "fas fa-trash"} aria-hidden="true" />
                              </button>
                            </div>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          )}

        </>
      )}

      {/* ── Modal ── */}
      {showAssignModal && (
        <AssignRoleModal
          assignment={editingAssignment}
          currentUserEmail={account?.email || account?.username || ""}
          onClose={() => { setShowAssignModal(false); setEditingAssignment(null); }}
          onSuccess={() => { setShowAssignModal(false); setEditingAssignment(null); loadAssignments(); }}
        />
      )}
    </div>
  );
}

interface AssignRoleModalProps {
  assignment: RoleAssignment | null;
  currentUserEmail: string;
  onClose: () => void;
  onSuccess: () => void;
}

function AssignRoleModal({ assignment, currentUserEmail, onClose, onSuccess }: AssignRoleModalProps) {
  const { account } = useAuth();
  const { gradientColors } = useTheme();
  const isEditing = !!assignment;
  const [email, setEmail] = useState(assignment?.user_email ?? "");
  const [role, setRole] = useState<UserRole>(assignment?.role ?? "Analyst");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const target = isEditing ? assignment!.user_email : email.trim().toLowerCase();
    if (!target) return;
    setSaving(true);
    setError(null);
    try {
      const res = await msalFetch(`${API_BASE}/api/v1/users/${encodeURIComponent(target)}/role`, {
        method: "PUT",
        headers: {
          "Content-Type": "application/json",
          "x-user-email": account?.email || account?.username || "",
        },
        body: JSON.stringify({ role }),
      });
      if (!res.ok) {
        const d = await res.json().catch(() => ({}));
        throw new Error(d?.detail?.message || d?.detail || `Failed (${res.status})`);
      }
      onSuccess();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to assign role");
    } finally {
      setSaving(false);
    }
  }

  const selectedMeta = ROLE_META[role];

  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)", zIndex: 1000 }}
        onClick={onClose}
      />
      <div style={{
        position: "fixed", top: "50%", left: "50%",
        transform: "translate(-50%, -50%)",
        background: "white", borderRadius: 14,
        boxShadow: "0 24px 64px rgba(0,0,0,0.25)",
        zIndex: 1001, width: "90%", maxWidth: 500,
        display: "flex", flexDirection: "column",
        overflow: "hidden",
      }}>
        {/* Gradient header bar */}
        <div style={{
          height: 4,
          background: `linear-gradient(90deg, ${gradientColors.light}, ${gradientColors.dark})`,
        }} />

        {/* Header */}
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1.25rem 1.5rem", borderBottom: "1px solid #f1f5f9" }}>
          <div>
            <h2 style={{ margin: 0, fontSize: "1.1rem", fontWeight: 700 }}>
              {isEditing ? "Edit Role Assignment" : "Assign Role"}
            </h2>
            {isEditing && (
              <p style={{ margin: "2px 0 0", fontSize: 12, color: "#6b7280", fontFamily: "monospace" }}>
                {assignment!.user_email}
              </p>
            )}
          </div>
          <button onClick={onClose} style={{ background: "none", border: "none", fontSize: "1.1rem", cursor: "pointer", color: "#94a3b8", width: 30, height: 30, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6 }}>
            <i className="fas fa-times" />
          </button>
        </div>

        <form onSubmit={handleSubmit}>
          <div style={{ padding: "1.25rem 1.5rem" }}>
            {error && (
              <div style={{ padding: "0.75rem 1rem", borderRadius: 8, marginBottom: "1rem", background: "#fef2f2", color: "#991b1b", border: "1px solid #fecaca", display: "flex", alignItems: "center", gap: "0.6rem", fontSize: 13 }}>
                <i className="fas fa-exclamation-circle" />
                {error}
              </div>
            )}

            {!isEditing && (
              <div className="chart-builder-field">
                <label className="chart-builder-label"><span>User email (UPN)</span></label>
                <input
                  type="email"
                  className="chart-builder-input"
                  placeholder="user@company.com"
                  value={email}
                  onChange={e => setEmail(e.target.value)}
                  required
                  autoFocus
                />
              </div>
            )}

            {/* Role picker as visual cards */}
            <div className="chart-builder-field">
              <label className="chart-builder-label"><span>Select Role</span></label>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginTop: 4 }}>
                {ROLES.map(r => {
                  const meta = ROLE_META[r];
                  const selected = role === r;
                  return (
                    <button
                      key={r}
                      type="button"
                      onClick={() => setRole(r)}
                      style={{
                        display: "flex", alignItems: "center", gap: "0.6rem",
                        padding: "0.65rem 0.85rem", borderRadius: 8, cursor: "pointer",
                        border: selected ? `2px solid ${meta.color}` : "2px solid #e5e7eb",
                        background: selected ? meta.bg : "white",
                        transition: "all 0.15s",
                        textAlign: "left",
                      }}
                    >
                      <div style={{
                        width: 28, height: 28, borderRadius: 7, flexShrink: 0,
                        background: selected ? meta.gradient : "#f1f5f9",
                        display: "flex", alignItems: "center", justifyContent: "center",
                        transition: "background 0.15s",
                      }}>
                        <i className={`fas ${meta.icon}`} style={{ fontSize: 11, color: selected ? "white" : "#94a3b8" }} />
                      </div>
                      <div>
                        <div style={{ fontSize: 13, fontWeight: 600, color: selected ? meta.color : "#374151" }}>{r}</div>
                      </div>
                      {selected && (
                        <i className="fas fa-check-circle" style={{ marginLeft: "auto", color: meta.color, fontSize: 14 }} />
                      )}
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Description of selected role */}
            <div style={{
              padding: "0.75rem 1rem", borderRadius: 8, marginTop: 4,
              background: selectedMeta.bg, border: `1px solid ${selectedMeta.border}`,
              display: "flex", alignItems: "flex-start", gap: "0.6rem",
            }}>
              <i className="fas fa-info-circle" style={{ color: selectedMeta.color, marginTop: 1, flexShrink: 0 }} />
              <p style={{ margin: 0, fontSize: 12, color: selectedMeta.color, lineHeight: 1.5 }}>
                {selectedMeta.description}
              </p>
            </div>
          </div>

          <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.75rem", padding: "1rem 1.5rem", borderTop: "1px solid #f1f5f9", background: "#f8fafc" }}>
            <Button type="button" variant="secondary" onClick={onClose} disabled={saving}>
              Cancel
            </Button>
            <Button type="submit" disabled={saving}>
              {saving
                ? <><i className="fas fa-spinner fa-spin" /> Saving…</>
                : <><i className="fas fa-check" /> {isEditing ? "Update Role" : "Assign Role"}</>
              }
            </Button>
          </div>
        </form>
      </div>
    </>
  );
}
