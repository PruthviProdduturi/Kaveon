"""Model Graph's persisted state across installer restarts."""
import importlib.util
import unittest
from copy import deepcopy
from pathlib import Path

spec = importlib.util.spec_from_file_location("install_entra", Path(__file__).with_name("install-entra.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)

GROUP = "11111111-1111-1111-1111-111111111111"
USER = "22222222-2222-2222-2222-222222222222"
CLIENT = "33333333-3333-3333-3333-333333333333"


class Directory:
    def __init__(self):
        self.app = {"id": "app", "appId": CLIENT}
        self.sp = None
        self.assignments = []
        self.consent = []
        self.membership_writes = 0

    def __call__(self, method, path, data=None, **kwargs):
        if path.startswith("groups/"):
            return {"securityEnabled": True}
        if path.startswith("users/"):
            return {"accountEnabled": True}
        if path.startswith("applications"):
            if method == "PATCH":
                self.app.update(deepcopy(data))
            return deepcopy(self.app)
        if path.startswith("servicePrincipals(appId"):
            return self.sp
        if path == "servicePrincipals" and method == "POST":
            self.sp = {"id": "sp", **data}
            return self.sp
        if path.startswith("oauth2PermissionGrants"):
            if method == "GET":
                return {"value": self.consent}
            self.consent.append(data)
            return
        if path.startswith("servicePrincipals/sp/appRoleAssignedTo"):
            if method == "GET":
                return {"value": self.assignments}
            self.assignments.append(data)
            self.membership_writes += 1
            return
        raise AssertionError((method, path))


class InstallTests(unittest.TestCase):
    def test_restarted_install_preserves_roles_callbacks_and_assignments(self):
        directory = Directory()
        origin = "https://studio.example.org"
        calls = []
        def interrupted(method, path, *args, **kwargs):
            if method == "POST" and path.endswith("appRoleAssignedTo") and not calls:
                calls.append(path)
                raise RuntimeError("interrupted")
            return directory(method, path, *args, **kwargs)
        with self.assertRaises(RuntimeError):
            installer.install(interrupted, origin, {GROUP: "Analyst"}, USER, True)
        directory.app["spa"]["redirectUris"].append("https://another.example.org/auth/microsoft")
        for _ in range(2):
            self.assertEqual(installer.install(directory, origin, {GROUP: "Analyst"}, USER, True), CLIENT)
        self.assertEqual(directory.membership_writes, 2)
        self.assertEqual(len(directory.consent), 1)
        self.assertEqual(directory.consent[0]["clientId"], directory.consent[0]["resourceId"])
        self.assertTrue(directory.sp["appRoleAssignmentRequired"])
        self.assertEqual({r["value"] for r in directory.app["appRoles"]}, {"Viewer", "Analyst", "Editor", "Admin"})
        self.assertEqual(len(directory.app["api"]["oauth2PermissionScopes"]), 1)
        self.assertEqual(len(directory.app["spa"]["redirectUris"]), 2)
        self.assertNotIn("passwordCredentials", directory.app)

    def test_catalog_group_gets_only_its_catalog_and_base_role(self):
        directory = Directory()
        for _ in range(2):
            installer.install(directory, "https://studio.example.org", {}, USER, bindings=[("OpenSource", GROUP)])
        by_id = {role["id"]: role["value"] for role in directory.app["appRoles"]}
        assigned = {by_id[item["appRoleId"]] for item in directory.assignments if item["principalId"] == GROUP}
        self.assertEqual(assigned, {"Analyst", "Kaveon.Query.OpenSource"})
        self.assertEqual(directory.membership_writes, 3)

    def test_checks_input_before_provisioning(self):
        for origin, groups, catalogs in [
            ("http://demo.example", ["Admin=" + GROUP], []),
            ("https://studio.example/other", ["Admin=" + GROUP], []),
            ("https://studio.example", ["Admin=" + GROUP], ["KaveonDB"]),
            ("https://studio.example", ["Analyst=" + GROUP], []),
        ]:
            with self.assertRaises(ValueError):
                installer.directory_inputs(origin, groups, catalogs)
