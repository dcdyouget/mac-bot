"""Exercise the generator CLI in a disposable repository, never real outputs."""
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest


class GeneratorCheckTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.script = self.root / "protocol/kotlin/generate.py"
        self.script.parent.mkdir(parents=True)
        shutil.copyfile(pathlib.Path(__file__).resolve().parents[1] / "generate.py", self.script)
        self.schema = self.root / "protocol/schema/hello.json"
        self.schema.parent.mkdir(parents=True)
        self.schema.write_text(json.dumps({
            "title": "Hello", "type": "object", "required": ["protocol"],
            "properties": {"protocol": {"type": "integer"}},
        }))
        self.fixture = self.root / "protocol/fixtures/objects/hello.json"
        self.fixture.parent.mkdir(parents=True)
        self.fixture.write_text('{"protocol":1,"future":{"retained":true}}')

    def run_generator(self, *arguments):
        return subprocess.run([sys.executable, str(self.script), *arguments], capture_output=True, text=True)

    def snapshot(self):
        return {str(path.relative_to(self.root)): (path.read_bytes(), path.stat().st_mtime_ns)
                for path in self.root.rglob("*") if path.is_file()}

    def test_fresh_outputs_check_without_rewriting(self):
        self.assertEqual(0, self.run_generator().returncode)
        before = self.snapshot()
        result = self.run_generator("--check")
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual(before, self.snapshot())

    def test_stale_models_corpus_and_inventory_fail_without_repair(self):
        self.assertEqual(0, self.run_generator().returncode)
        outputs = [
            self.root / "clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile/core/protocol/generated/SchemaModels.kt",
            self.root / "clients/mobile/shared/src/androidUnitTest/resources/protocol-fixtures.json",
            self.root / "protocol/kotlin/schema-inventory.json",
        ]
        for path in outputs:
            with self.subTest(output=path.name):
                original = path.read_text()
                path.write_text("stale output")
                before = self.snapshot()
                result = self.run_generator("--check")
                self.assertNotEqual(0, result.returncode)
                self.assertIn(str(path.relative_to(self.root)), result.stderr)
                self.assertEqual(before, self.snapshot())
                path.write_text(original)

    def test_missing_output_fails_without_creating_directories(self):
        before = self.snapshot()
        result = self.run_generator("--check")
        self.assertNotEqual(0, result.returncode)
        self.assertEqual(before, self.snapshot())
        self.assertFalse((self.root / "clients").exists())

    def test_missing_inputs_fail_without_overwriting_existing_outputs(self):
        self.assertEqual(0, self.run_generator().returncode)
        for path in (self.schema, self.fixture):
            with self.subTest(input=path.name):
                original = path.read_text()
                path.unlink()
                before = self.snapshot()
                result = self.run_generator("--check")
                self.assertNotEqual(0, result.returncode)
                self.assertIn("missing", result.stderr)
                self.assertEqual(before, self.snapshot())
                path.write_text(original)

    def test_invalid_fixture_does_not_partially_update_outputs(self):
        self.assertEqual(0, self.run_generator().returncode)
        self.fixture.write_text("invalid JSON")
        self.schema.write_text('{"title":"Changed","type":"object","properties":{}}')
        before = self.snapshot()
        result = self.run_generator()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("Unable to read protocol fixtures", result.stderr)
        self.assertEqual(before, self.snapshot())


if __name__ == "__main__":
    unittest.main()
