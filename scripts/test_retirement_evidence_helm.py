"""Static Helm contract for durable PostgreSQL-retirement evidence storage."""
from pathlib import Path


ROOT = Path(__file__).parents[1] / "infra" / "helm" / "kaveon-portal-test"


def test_evidence_pvc_is_opt_in_durable_and_rwx_capable():
    template = (ROOT / "templates" / "retirement-evidence-pvc.yaml").read_text("utf-8")
    values = (ROOT / "values.yaml").read_text("utf-8")
    assert "if .Values.api.evidenceStorage.create" in template
    assert "helm.sh/resource-policy: keep" in template
    assert "storageClassName:" in template
    assert "accessModes:" in template
    assert "create: false" in values
    assert "accessModes: [ReadWriteMany]" in values


def test_restart_api_reads_same_durable_evidence_paths():
    api = (ROOT / "templates" / "api.yaml").read_text("utf-8")
    required = (
        "KAVEON_POSTGRESQL_RESTART_REHEARSAL_MODE",
        "KAVEON_POSTGRESQL_RECONCILIATION_REPORTS",
        "KAVEON_POSTGRESQL_OPERATIONAL_RECEIPTS",
        "mountPath: /retirement, readOnly: true",
        "persistentVolumeClaim: {claimName: {{ .Values.api.cutover.evidencePvc }}",
    )
    for value in required:
        assert value in api


def test_example_uses_one_claim_for_every_evidence_consumer():
    example = (ROOT / "examples" / "postgresql-free.values.yaml").read_text("utf-8")
    assert example.count("evidencePvc: kaveon-retirement-evidence") == 3
    assert "name: kaveon-retirement-evidence" in example
