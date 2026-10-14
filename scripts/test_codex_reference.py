#!/usr/bin/env python3
"""Offline wrapper regression; never downloads or executes the Codex artifact."""

import importlib.util
from pathlib import Path
import socket
import tempfile
import tomllib
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "images/codex/entrypoint.py"
SPEC = importlib.util.spec_from_file_location("codex_reference_entrypoint", SOURCE)
ENTRYPOINT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ENTRYPOINT)


class CodexReference(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.state = self.root / "state"
        self.temporary = self.root / "tmp"
        self.state.mkdir(mode=0o700)
        self.temporary.mkdir(mode=0o700)
        self.environment = {
            "OPENAI_API_KEY": "a" * 64,
            "TENTAFLAKE_BROKER_TOKEN": "a" * 64,
            "OPENAI_BASE_URL": "http://10.88.0.1:8080/v1",
            "TENTAFLAKE_LLM_BASE_URL": "http://10.88.0.1:8080/v1",
            "PATH": "/bin",
        }

    def prepare(self, model="fixture-model"):
        return ENTRYPOINT.prepare(self.environment, model, self.state, self.temporary)

    def test_provider_config_parses_without_a_secret_or_environment_interpolation(self):
        arguments, environment = self.prepare()
        config = tomllib.loads("\n".join(
            arguments[index + 1]
            for index, value in enumerate(arguments)
            if value == "-c"
        ))
        self.assertEqual(config["model"], "fixture-model")
        self.assertEqual(config["model_provider"], "tentaflake")
        self.assertEqual(config["model_providers"]["tentaflake"], {
            "name": "Tentaflake broker",
            "base_url": self.environment["OPENAI_BASE_URL"],
            "env_key": "OPENAI_API_KEY",
            "wire_api": "responses",
        })
        self.assertNotIn(self.environment["OPENAI_API_KEY"], " ".join(arguments))
        self.assertEqual(environment["OPENAI_API_KEY"], self.environment["OPENAI_API_KEY"])
        self.assertEqual(environment["CODEX_HOME"], str(self.state / ".codex"))
        self.assertEqual(arguments[-4:], [
            "app-server", "--strict-config", "--listen",
            f"unix://{self.temporary}/tentaflake-codex/server.sock",
        ])
        self.assertNotIn("CODEX_HOME", self.environment)
        self.assertFalse((self.state / ".codex/config.toml").exists())

    def test_restart_preserves_config_and_history_inodes_and_bytes(self):
        self.prepare()
        home = self.state / ".codex"
        config = home / "config.toml"
        history = home / "history.jsonl"
        config.write_text('file_opener = "none"\n')
        history.write_text('{"session":"kept"}\n')
        previous = [(path.stat().st_ino, path.read_bytes()) for path in (config, history)]
        next_temporary = self.root / "next-tmp"
        next_temporary.mkdir(mode=0o700)
        ENTRYPOINT.prepare(self.environment, "fixture-model", self.state, next_temporary)
        self.assertEqual(previous, [
            (path.stat().st_ino, path.read_bytes()) for path in (config, history)
        ])
        self.assertEqual(home.stat().st_mode & 0o777, 0o700)

    def test_invalid_credentials_and_urls_refuse_before_state_creation(self):
        invalid = [
            {"OPENAI_API_KEY": "real-provider-like-secret"},
            {"TENTAFLAKE_BROKER_TOKEN": "b" * 64},
            {"TENTAFLAKE_LLM_BASE_URL": "http://10.88.0.1:8081/v1"},
        ]
        for url in (
            "https://10.88.0.1:8080/v1", "http://127.0.0.1:8080/v1",
            "http://8.8.8.8:8080/v1", "http://host:8080/v1",
            "http://user:secret@10.88.0.1:8080/v1", "http://10.88.0.1:0/v1",
            "http://10.88.0.1:8080/v1/", "http://10.88.0.1:8080/v1?secret",
            "http://10.88.0.1:8080/v1#secret", "http://10.88.0.1:8080/v1\n",
        ):
            invalid.append({"OPENAI_BASE_URL": url, "TENTAFLAKE_LLM_BASE_URL": url})
        for change in invalid:
            with self.subTest(change=list(change)):
                environment = self.environment | change
                with self.assertRaisesRegex(ValueError, "^invalid broker environment$"):
                    ENTRYPOINT.prepare(
                        environment, "fixture-model", self.state, self.temporary
                    )
                self.assertEqual(list(self.state.iterdir()), [])

    def test_no_default_or_injected_model(self):
        for model in ("", "model\nother=true", 'model"', "-model", "x" * 129):
            with self.subTest(model_length=len(model)), self.assertRaises(ValueError):
                self.prepare(model)
        self.assertEqual(list(self.state.iterdir()), [])

    def test_state_symlink_or_shared_directory_is_never_repaired(self):
        other = self.root / "other"
        other.mkdir(mode=0o700)
        home = self.state / ".codex"
        home.symlink_to(other)
        with self.assertRaises(OSError):
            self.prepare()
        self.assertTrue(home.is_symlink())
        self.assertEqual(list(other.iterdir()), [])
        home.unlink()
        home.mkdir(mode=0o755)
        home.chmod(0o755)
        with self.assertRaises(ValueError):
            self.prepare()
        self.assertEqual(home.stat().st_mode & 0o777, 0o755)

    def test_existing_server_socket_remains_bound_and_untouched(self):
        self.prepare()
        path = self.temporary / "tentaflake-codex/server.sock"
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(path))
            listener.listen(1)
            identity = path.stat().st_ino
            with self.assertRaisesRegex(ValueError, "server socket already exists"):
                self.prepare()
            self.assertEqual(path.stat().st_ino, identity)
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(1)
                client.connect(str(path))
                accepted, _ = listener.accept()
                accepted.close()

    def test_existing_socket_alias_is_not_followed_or_deleted(self):
        self.prepare()
        path = self.temporary / "tentaflake-codex/server.sock"
        path.symlink_to(self.root / "absent")
        with self.assertRaises(ValueError):
            self.prepare()
        self.assertTrue(path.is_symlink())
        self.assertFalse((self.root / "absent").exists())


if __name__ == "__main__":
    unittest.main()
