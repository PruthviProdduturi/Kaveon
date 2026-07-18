export async function fetchHealth() {
  const res = await fetch(`/health`);
  if (!res.ok) {
    throw new Error(`Health check failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchApiHealth() {
  const res = await fetch(`/api/v1/health`);
  if (!res.ok) {
    throw new Error(`API health failed: ${res.status}`);
  }
  return res.json();
}
