import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import egg_desktop


class RemoteCommandTests(unittest.TestCase):
    def test_remote_status_prints_current_config(self) -> None:
        config = {
            "enabled": True,
            "server_url": "https://eggs.example.com",
            "mode": "room",
            "room": "ABC123",
            "sprite": "dino",
        }
        with patch.object(egg_desktop, "read_remote_config", return_value=config):
            out = io.StringIO()
            with patch("sys.stdout", out):
                code = egg_desktop.remote_command("status")
        self.assertEqual(code, 0)
        self.assertIn("remote enabled=True server=https://eggs.example.com mode=room room=ABC123 sprite=dino", out.getvalue())

    def test_remote_without_action_defaults_to_random(self) -> None:
        with (
            patch.object(egg_desktop, "read_sprite", return_value="dino"),
            patch.object(egg_desktop, "ensure_remote_sprite_uploaded", return_value=True) as upload,
            patch.object(egg_desktop, "write_remote_config") as write_remote_config,
            patch.object(egg_desktop, "apply_remote_runtime_change") as apply_remote_runtime_change,
        ):
            out = io.StringIO()
            with patch("sys.stdout", out):
                code = egg_desktop.remote_command(None)
        self.assertEqual(code, 0)
        upload.assert_called_once_with("dino", quiet=True)
        write_remote_config.assert_called_once_with({"enabled": True, "mode": "random", "room": "", "sprite": "dino"})
        apply_remote_runtime_change.assert_called_once_with(ensure_running=True)
        self.assertIn("remote random match pool enabled", out.getvalue())

    def test_remote_random_upload_failure_returns_error(self) -> None:
        with (
            patch.object(egg_desktop, "read_sprite", return_value="dino"),
            patch.object(egg_desktop, "ensure_remote_sprite_uploaded", return_value=False),
            patch.object(egg_desktop, "write_remote_config") as write_remote_config,
            patch.object(egg_desktop, "apply_remote_runtime_change") as apply_remote_runtime_change,
        ):
            err = io.StringIO()
            with patch("sys.stderr", err):
                code = egg_desktop.remote_command(None)
        self.assertEqual(code, 1)
        self.assertIn("remote random match pool not enabled", err.getvalue())
        write_remote_config.assert_not_called()
        apply_remote_runtime_change.assert_not_called()

    def test_remote_room_switches_mode_and_restarts_sidecar(self) -> None:
        with (
            patch.object(egg_desktop, "read_sprite", return_value="dino"),
            patch.object(egg_desktop, "ensure_remote_sprite_uploaded", return_value=True),
            patch.object(egg_desktop, "write_remote_config") as write_remote_config,
            patch.object(egg_desktop, "apply_remote_runtime_change") as apply_remote_runtime_change,
        ):
            out = io.StringIO()
            with patch("sys.stdout", out):
                code = egg_desktop.remote_command("room", "ABC123")
        self.assertEqual(code, 0)
        write_remote_config.assert_called_once_with({"enabled": True, "mode": "room", "room": "ABC123", "sprite": "dino"})
        apply_remote_runtime_change.assert_called_once_with(ensure_running=True)
        self.assertIn("remote room enabled: ABC123", out.getvalue())

    def test_remote_leave_disables_remote_and_restarts_sidecar(self) -> None:
        with (
            patch.object(egg_desktop, "write_remote_config") as write_remote_config,
            patch.object(egg_desktop, "apply_remote_runtime_change") as apply_remote_runtime_change,
        ):
            out = io.StringIO()
            with patch("sys.stdout", out):
                code = egg_desktop.remote_command("leave")
        self.assertEqual(code, 0)
        write_remote_config.assert_called_once_with({"enabled": False, "mode": "random", "room": ""})
        apply_remote_runtime_change.assert_called_once_with()
        self.assertIn("left remote interaction", out.getvalue())


class RemoteStateTests(unittest.TestCase):
    def test_remote_error_is_permanent(self) -> None:
        self.assertTrue(egg_desktop.remote_error_is_permanent("HTTP/1.1 400 Bad Request"))
        self.assertTrue(egg_desktop.remote_error_is_permanent("unknown sprite for device"))
        self.assertFalse(egg_desktop.remote_error_is_permanent("websocket closed"))

    def test_stop_remote_sidecar_clears_remote_peers_snapshot(self) -> None:
        with tempfile.TemporaryDirectory(prefix="egg-desktop-test-") as tmpdir:
            app_dir = Path(tmpdir)
            with (
                patch.object(egg_desktop, "APP_DIR", app_dir),
                patch.object(egg_desktop, "REMOTE_PID_FILE", app_dir / "remote.pid"),
                patch.object(egg_desktop, "REMOTE_PEERS_FILE", app_dir / "remote-peers.json"),
                patch.object(egg_desktop, "read_remote_pid", return_value=None),
                patch.object(egg_desktop, "managed_remote_process_alive", return_value=False),
                patch.object(egg_desktop, "remote_enabled", return_value=True),
            ):
                code = egg_desktop.stop_remote_sidecar()
                self.assertEqual(code, 0)
                snapshot = json.loads((app_dir / "remote-peers.json").read_text(encoding="utf-8"))
        self.assertEqual(
            snapshot,
            {
                "enabled": True,
                "connected": False,
                "reconnecting": False,
                "error": "",
                "peers": [],
            },
        )


if __name__ == "__main__":
    unittest.main()
