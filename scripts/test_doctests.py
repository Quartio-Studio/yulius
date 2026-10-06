"""Doctest selection must start covering libraries as the workspace grows."""
import unittest
from doctests import library_packages


class DoctestSelection(unittest.TestCase):
    def test_binary_only_is_not_applicable(self):
        data = {"workspace_members": ["app"], "packages": [{"id": "app", "name": "app", "targets": [{"kind": ["bin"], "doctest": False}]}]}
        self.assertEqual(library_packages(data), [])

    def test_library_in_mixed_workspace_selected_not_external_dependency(self):
        data = {"workspace_members": ["core", "app", "macros"], "packages": [
            {"id": "core", "name": "core", "targets": [{"kind": ["lib"], "doctest": True}]},
            {"id": "app", "name": "app", "targets": [{"kind": ["bin"], "doctest": False}]},
            {"id": "macros", "name": "macros", "targets": [{"kind": ["proc-macro"], "doctest": True}]},
            {"id": "external", "name": "external", "targets": [{"kind": ["lib"], "doctest": True}]}]}
        self.assertEqual(library_packages(data), ["core", "macros"])
