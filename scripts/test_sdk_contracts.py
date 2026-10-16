"""Check the shared catalog's support boundaries and generation invariants."""

import copy
import json
import unittest
from unittest.mock import patch

import sdk_contracts as contracts
from generate_python_sdk import lift_nested_models, py_type


class CatalogTests(unittest.TestCase):
    def test_sdk_parity_preserves_hosted_support_boundary(self):
        baseline = {route["handler"] for route in contracts.read_contracts()[0]}
        self.assertEqual(len(baseline), 12)
        rust = contracts.read_contracts("rust")[0]
        self.assertEqual(len(rust), 22)
        for sdk in ("python", "typescript", "swift", "go"):
            self.assertEqual(contracts.read_contracts(sdk)[0], rust)
            self.assertEqual(contracts.runtime_contracts(sdk), contracts.runtime_contracts("rust"))
        self.assertNotIn("metadataUpdates", contracts.runtime_contracts("worker"))

    def test_unknown_runtime_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Unknown SDK runtime"):
            contracts.read_contracts("pyhton")

    def test_duplicate_operation_ids_are_rejected_before_json_overwrites_them(self):
        with self.assertRaisesRegex(ValueError, "Duplicate SDK contract key: attachBuild"):
            json.loads('{"attachBuild": {}, "attachBuild": {}}', object_pairs_hook=contracts.unique_object)

    def test_duplicate_route_ids_are_rejected(self):
        catalog = copy.deepcopy(contracts.CATALOG)
        catalog["readGroups"].append({"name": "duplicate", "source": "read-contracts.json", "implementations": ["rust"]})
        with patch.object(contracts, "CATALOG", catalog), self.assertRaisesRegex(ValueError, "Duplicate SDK read handler or route"):
            contracts.validate_catalog()

    def test_unknown_operation_models_are_rejected(self):
        catalog = copy.deepcopy(contracts.CATALOG)
        catalog["operations"]["attachBuild"]["result"] = "UnknownModel"
        with patch.object(contracts, "CATALOG", catalog), self.assertRaisesRegex(ValueError, "Unknown SDK operation model"):
            contracts.validate_catalog()

    def test_unknown_nested_model_refs_are_rejected(self):
        catalog = copy.deepcopy(contracts.CATALOG)
        catalog["operationModels"]["ListingImageUpload"]["properties"]["placementType"]["$ref"] = "#/components/schemas/Missing"
        with patch.object(contracts, "CATALOG", catalog), self.assertRaisesRegex(ValueError, "Unknown model reference"):
            contracts.validate_catalog()

    def test_ref_resolution_preserves_nullable_constraints(self):
        self.assertEqual(
            contracts.resolve_schema({"$ref": "#/components/schemas/Name", "nullable": True}, {"Name": {"type": "string"}}),
            {"type": "string", "nullable": True},
        )

    def test_python_response_arrays_differ_from_query_selections(self):
        selection = {"type": "array", "items": {"type": "string", "enum": ["COMPLETE"]}}
        self.assertEqual(py_type(selection, {}, response=True, optional=True), "list[str] | None")
        self.assertEqual(py_type(selection, {}), "str | Sequence[Literal['COMPLETE']]")
        nested = lift_nested_models({"Build": {"type": "object", "properties": {"icon": {"type": "object", "properties": {"width": {"type": "integer"}}}}}})
        self.assertIn("BuildIcon", nested)
        self.assertEqual(py_type(nested["Build"]["properties"]["icon"], nested, response=True), "BuildIcon")


if __name__ == "__main__":
    unittest.main()
