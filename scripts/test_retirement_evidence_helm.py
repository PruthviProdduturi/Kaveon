"""Static Helm contract for durable PostgreSQL-retirement evidence storage."""
from pathlib import Path


# The chart was renamed kaveon-portal-test -> kaveon-platform and this path was
# not. The test then failed on a missing file rather than on a broken contract,
# and it failed only in CI: a pytest run from `api/` never collects `scripts/`,
# so the whole suite looked green locally while the pipeline stayed red.
ROOT = Path(__file__).parents[1] / "infra" / "helm" / "kaveon-platform"


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
    """The API's evidence contract, wherever the chart happens to declare it.

    The three environment variables moved out of `api.yaml` into the shared
    `portal.apiEnv` helper, so pinning this to one file asserted the layout of
    the chart rather than the contract, and failed on a move that changed
    nothing. The mount and the claim stay in the pod spec, so those are still
    read from `api.yaml`; the variables are looked for across the API's whole
    declared environment.
    """
    api = (ROOT / "templates" / "api.yaml").read_text("utf-8")
    helpers = (ROOT / "templates" / "_helpers.tpl").read_text("utf-8")
    environment = api + helpers
    for value in (
        "KAVEON_POSTGRESQL_RESTART_REHEARSAL_MODE",
        "KAVEON_POSTGRESQL_RECONCILIATION_REPORTS",
        "KAVEON_POSTGRESQL_OPERATIONAL_RECEIPTS",
    ):
        assert value in environment, f"{value} is declared nowhere in the API env"
    for value in (
        "mountPath: /retirement, readOnly: true",
        "persistentVolumeClaim: {claimName: {{ .Values.api.cutover.evidencePvc }}",
    ):
        assert value in api, f"{value} is not in the API pod spec"


def test_example_uses_one_claim_for_every_evidence_consumer():
    example = (ROOT / "examples" / "postgresql-free.values.yaml").read_text("utf-8")
    assert example.count("evidencePvc: kaveon-retirement-evidence") == 3
    assert "name: kaveon-retirement-evidence" in example
