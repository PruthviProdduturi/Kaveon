"""Provision the tenant-owned sign-in application for one Kaveon installation."""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode, urlsplit
from urllib.request import Request, urlopen
from uuid import UUID, uuid5, NAMESPACE_URL

GRAPH = "https://graph.microsoft.com/v1.0/"
ROLES = json.loads((Path(__file__).resolve().parents[1] / "api/services/entra_roles.json").read_text())
ROLE_IDS = {r["value"]: r["id"] for r in ROLES}


def graph(token, method, path, data=None, *, headers=None, missing=False):
    url = path if path.startswith(GRAPH) else GRAPH + path
    request = Request(url, method=method, data=json.dumps(data).encode() if data is not None else None,
                      headers={"Authorization": "Bearer " + token, "Content-Type": "application/json", **(headers or {})})
    for attempt in range(4):
        try:
            with urlopen(request, timeout=30) as response:
                body = response.read()
                return json.loads(body) if body else None
        except HTTPError as error:
            if missing and error.code == 404:
                return None
            if error.code == 429 and attempt < 3:
                time.sleep(min(60, int(error.headers.get("Retry-After", "5"))))
                continue
            raise SystemExit(f"Microsoft Graph {method} {urlsplit(url).path} returned HTTP {error.code}. "
                             "Check directory permissions, then rerun with the same origin.") from None


def directory_inputs(origin, pairs, catalogs, initial_admin=None):
    url = urlsplit(origin)
    if url.scheme != "https" or not url.hostname or url.username or url.password or url.port not in (None, 443) or url.path not in ("", "/") or url.query or url.fragment:
        raise ValueError("Supply the public HTTPS Studio origin.")
    if not re.fullmatch(r"[a-zA-Z0-9.-]+", url.hostname):
        raise ValueError("Invalid Studio hostname.")
    assignments = {}
    for pair in pairs:
        name, sep, group = pair.partition("=")
        if not sep or name not in ROLE_IDS:
            raise ValueError("Assignments use Role=group-object-id.")
        group = str(UUID(group))
        if group in assignments and assignments[group] != name:
            raise ValueError("A group was supplied with conflicting roles.")
        assignments[group] = name
    if "Admin" not in assignments.values() and not initial_admin:
        raise ValueError("Specify an Admin security group or an initial admin user object ID.")
    if any(not re.fullmatch(r"[A-Za-z0-9_]{1,64}", c) or c.lower() == "kaveondb" for c in catalogs):
        raise ValueError("Use registered lake catalog names.")
    return "https://" + url.hostname.lower(), assignments


def catalog_bindings(pairs):
    bindings = set()
    for pair in pairs:
        catalog, sep, group = pair.partition("=")
        if not sep or not re.fullmatch(r"[A-Za-z0-9_]{1,64}", catalog) or catalog.lower() == "kaveondb":
            raise ValueError("Catalog bindings use lake-catalog=group-object-id.")
        bindings.add((catalog, str(UUID(group))))
    return sorted(bindings)


def install(call, origin, groups, initial_admin=None, grant_consent=False, bindings=()):
    # A tenant-scoped immutable key makes an interrupted creation retryable.
    key = "kaveon-" + hashlib.sha256(origin.encode()).hexdigest()[:24]
    app_path = f"applications(uniqueName='{key}')"
    assignments = dict(groups)
    for _, group in bindings:
        assignments.setdefault(group, "Analyst")
    for group in assignments:
        if not call("GET", "groups/" + group + "?$select=id,securityEnabled").get("securityEnabled"):
            raise ValueError(f"{group} is not a security group in the selected tenant.")
    if initial_admin:
        user = call("GET", "users/" + initial_admin + "?$select=id,accountEnabled")
        if user.get("accountEnabled") is not True:
            raise ValueError("The initial administrator must be an enabled user in this directory.")
        assignments[initial_admin] = "Admin"
    call("PATCH", app_path, {"displayName": "Kaveon · " + urlsplit(origin).hostname},
         headers={"Prefer": "create-if-missing"})
    app = call("GET", app_path)
    client = app["appId"]
    scope_id = str(uuid5(NAMESPACE_URL, "api://" + client + "/access_as_user"))
    api = app.get("api") or {}
    scopes = list(api.get("oauth2PermissionScopes") or [])
    existing = next((s for s in scopes if s["value"] == "access_as_user"), None)
    if existing and not existing.get("isEnabled"):
        raise ValueError("The access_as_user scope has been disabled in Entra.")
    if not existing:
        scopes.append({"id": scope_id, "value": "access_as_user", "isEnabled": True, "type": "User",
                       "adminConsentDisplayName": "Access Kaveon as the signed-in user",
                       "userConsentDisplayName": "Access Kaveon on your behalf",
                       "adminConsentDescription": "Use the user's assigned Kaveon role.",
                       "userConsentDescription": "Use your assigned Kaveon role."})
    roles = list(app.get("appRoles") or [])
    role_ids = {}
    scoped_roles = []
    for catalog in sorted({catalog for catalog, _ in bindings}):
        value = "Kaveon.Query." + catalog
        scoped_roles.append({"value": value, "displayName": "Query " + catalog, "description": "Query the " + catalog + " lake catalog",
                             "id": str(uuid5(NAMESPACE_URL, client + "/" + value)), "isEnabled": True, "allowedMemberTypes": ["User"]})
    for role in ROLES + scoped_roles:
        found = next((r for r in roles if r["value"] == role["value"]), None)
        if found and (not found.get("isEnabled") or "User" not in found["allowedMemberTypes"]):
            raise ValueError(f"Unusable existing role {role['value']}.")
        if not found:
            roles.append(role)
        role_ids[role["value"]] = (found or role)["id"]
    def with_value(values, value):
        return sorted(set(values or []) | {value})
    # Explicitly declare the delegated permission the public Studio client requests.
    required = list(app.get("requiredResourceAccess") or [])
    resource = next((r for r in required if r["resourceAppId"] == client), None)
    if resource is None:
        resource = {"resourceAppId": client, "resourceAccess": []}
        required.append(resource)
    access = {"id": existing["id"] if existing else scope_id, "type": "Scope"}
    if access not in resource["resourceAccess"]:
        resource["resourceAccess"].append(access)
    call("PATCH", "applications/" + app["id"], {
        "signInAudience": "AzureADMyOrg", "appRoles": roles, "isFallbackPublicClient": True,
        "identifierUris": with_value(app.get("identifierUris"), "api://" + client),
        "api": {"requestedAccessTokenVersion": 2, "oauth2PermissionScopes": scopes},
        "spa": {"redirectUris": with_value((app.get("spa") or {}).get("redirectUris"), origin + "/auth/microsoft")},
    })
    call("PATCH", "applications/" + app["id"], {"requiredResourceAccess": required})
    sp = call("GET", f"servicePrincipals(appId='{client}')", missing=True)
    if not sp:
        sp = call("POST", "servicePrincipals", {"appId": client, "appRoleAssignmentRequired": True})
    sp_path = "servicePrincipals/" + sp["id"]
    if not sp.get("appRoleAssignmentRequired"):
        call("PATCH", sp_path, {"appRoleAssignmentRequired": True})
    if grant_consent:
        # Only this client calling its own Kaveon resource, never Microsoft Graph.
        sp_id = sp["id"]
        query = urlencode({"$filter": f"clientId eq '{sp_id}' and resourceId eq '{sp_id}' and consentType eq 'AllPrincipals'"})
        consents = call("GET", "oauth2PermissionGrants?" + query)["value"]
        if not consents:
            call("POST", "oauth2PermissionGrants", {"clientId": sp_id, "resourceId": sp_id,
                 "consentType": "AllPrincipals", "scope": "access_as_user"})
        elif "access_as_user" not in consents[0]["scope"].split():
            call("PATCH", "oauth2PermissionGrants/" + consents[0]["id"],
                 {"scope": consents[0]["scope"] + " access_as_user"})
    path = sp_path + "/appRoleAssignedTo"
    page = call("GET", path + "?$select=principalId,appRoleId")
    assigned = set()
    while page:
        assigned.update((a["principalId"], a["appRoleId"]) for a in page["value"])
        link = page.get("@odata.nextLink")
        if link and not link.startswith(GRAPH):
            raise ValueError("Unexpected directory pagination origin.")
        page = call("GET", link) if link else None
    requested = set(assignments.items()) | {(group, "Kaveon.Query." + catalog) for catalog, group in bindings}
    for group, role in sorted(requested):
        if (group, role_ids[role]) not in assigned:
            call("POST", path, {"principalId": group, "resourceId": sp["id"], "appRoleId": role_ids[role]})
    return client


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--tenant", required=True, type=UUID)
    p.add_argument("--origin", required=True)
    p.add_argument("--group", action="append", default=[], metavar="Role=object-id")
    p.add_argument("--catalog-group", action="append", default=[], metavar="Catalog=group-object-id", help="Grant access to only this catalog through an Entra group")
    p.add_argument("--admin-user", type=UUID, help="Initial admin user object ID when a group is not available")
    p.add_argument("--catalog", action="append", default=[])
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--grant-consent", action="store_true", help="Consent to this app accessing its own Kaveon API for assigned users")
    p.add_argument("--apply", action="store_true")
    args = p.parse_args()
    origin, groups = directory_inputs(args.origin, args.group, args.catalog, args.admin_user)
    bindings = catalog_bindings(args.catalog_group)
    tenant = str(args.tenant)
    if not args.apply:
        print(json.dumps({"tenant": tenant, "origin": origin, "redirect": origin + "/auth/microsoft",
                          "grantKaveonDelegatedConsent": args.grant_consent, "initialAdmin": str(args.admin_user) if args.admin_user else None, "roles": list(ROLE_IDS), "groups": groups, "sharedCatalogs": args.catalog, "catalogGroups": bindings}, indent=2))
        return
    az = shutil.which("az")
    if not az:
        p.error("Install the Azure CLI and sign in to the target tenant.")
    result = json.loads(subprocess.check_output([az, "account", "get-access-token", "--tenant", tenant,
                        "--resource-type", "ms-graph", "--output", "json"], text=True))
    if result["tenant"].lower() != tenant:
        p.error("Azure CLI returned a token from a different directory.")
    client = install(lambda *a, **kw: graph(result["accessToken"], *a, **kw), origin, groups, str(args.admin_user) if args.admin_user else None, args.grant_consent, bindings)
    config = {"tenantId": tenant, "clientId": client}
    values = {"entraAccess": {**config, "catalogs": sorted(set(args.catalog))},
              "portal": {"publicUrl": origin, "entraPublicClient": True, "entraAccess": config}}
    args.output.write_text(json.dumps(values, indent=2) + "\n", encoding="utf-8")
    print(f"Tenant registration ready. Non-secret Helm values saved to {args.output}.")


if __name__ == "__main__":
    main()
