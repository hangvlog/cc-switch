#!/usr/bin/env python3
"""Persist the existing macOS bridge through Codex's login-shell environment load."""
import argparse
import datetime
import os
from pathlib import Path
import shlex
import stat
import tempfile

BEGIN = "# >>> ClawKit Codex shell bridge >>>"
END = "# <<< ClawKit Codex shell bridge <<<"


def write_atomic(path, content, mode):
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            os.fchmod(stream.fileno(), mode)
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def unmarked(text):
    if BEGIN not in text and END not in text:
        return text
    if text.count(BEGIN) != 1 or text.count(END) != 1:
        raise ValueError("Ambiguous managed block; no changes made")
    start = text.index(BEGIN)
    end = text.index(END) + len(END)
    if end < start:
        raise ValueError("Invalid managed block; no changes made")
    if end < len(text) and text[end] == "\n":
        end += 1
    return text[:start] + text[end:]


def shell_hook(adapter, engine, preference):
    # The marker narrows this to Codex's environment collection, excluding normal
    # terminals and later tool shells. No daemon, GUI, or other process is started.
    return f"""# Managed by configure-codex-shell-bridge.py; zsh only.
() {{
  [[ "${{CODEX_SHELL:-}}" == 1 ]] || return 0
  [[ "${{ZSH_EXECUTION_STRING:-}}" == *'_SHELL_ENV_DELIMITER_'* && "${{ZSH_EXECUTION_STRING:-}}" == *'command env -0'* ]] || return 0
  local adapter={shlex.quote(str(adapter))}
  local engine={shlex.quote(str(engine))}
  local preference={shlex.quote(str(preference))}
  [[ -r "$preference" && "$(<"$preference")" == true ]] || return 0
  [[ -x "$adapter" && -x "$engine" ]] || return 0
  [[ -z "${{CODEX_CLI_PATH:-}}" || "$CODEX_CLI_PATH" == "$adapter" ]] || return 0
  [[ -z "${{CLAWKIT_CODEX_ENGINE:-}}" || "$CLAWKIT_CODEX_ENGINE" == "$engine" ]] || return 0
  export CLAWKIT_CODEX_ENGINE="$engine"
  export CODEX_CLI_PATH="$adapter"
}}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["install", "uninstall"])
    parser.add_argument("--home", type=Path, default=Path.home())
    parser.add_argument("--shell-dir", type=Path)
    parser.add_argument("--storage", type=Path)
    parser.add_argument("--adapter", type=Path,
                        default=Path("/Applications/ClawKit Desktop.app/Contents/MacOS/cc-switch"))
    parser.add_argument("--engine", type=Path)
    args = parser.parse_args()
    home = args.home.expanduser().resolve()
    shell_dir = (args.shell_dir or Path(os.environ.get("ZDOTDIR", home))).expanduser().resolve()
    storage = (args.storage or home / ".cc-switch/desktop-remote").expanduser().resolve()
    rc = shell_dir / ".zshrc"
    hook = storage / "ordinary-launch.zsh"
    for path in [rc, hook]:
        if path.is_symlink() or (path.exists() and (not path.is_file() or path.stat().st_uid != os.getuid())):
            raise ValueError(f"Refusing to replace non-owned or linked file: {path}")
    original = rc.read_text() if rc.exists() else ""
    cleaned = unmarked(original)
    if args.action == "install":
        engine = args.engine or next((Path(app) / "Contents/Resources/codex-cli/bin/codex"
            for app in ["/Applications/ChatGPT.app", "/Applications/Codex.app"]
            if (Path(app) / "Contents/Resources/codex-cli/bin/codex").is_file()), None)
        if engine is None or any(not p.is_absolute() or not os.access(p, os.X_OK)
                                 for p in [args.adapter, engine]):
            raise ValueError("Installed executable adapter and official engine are required")
        storage.mkdir(parents=True, exist_ok=True, mode=0o700)
        if storage.stat().st_uid != os.getuid():
            raise ValueError("Configuration directory belongs to another user")
        write_atomic(hook, shell_hook(args.adapter, engine, storage / "enabled.json"), 0o600)
        block = f'{BEGIN}\n[[ ! -r {shlex.quote(str(hook))} ]] || source {shlex.quote(str(hook))}\n{END}\n'
        desired = cleaned + ("\n" if cleaned and not cleaned.endswith("\n") else "") + block
    else:
        desired = cleaned
    if desired != original:
        shell_dir.mkdir(parents=True, exist_ok=True)
        mode = stat.S_IMODE(rc.stat().st_mode) if rc.exists() else 0o600
        if rc.exists():
            stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%S%f")
            backup = rc.with_name(f".zshrc.clawkit-backup-{stamp}")
            write_atomic(backup, original, 0o600)
            print(f"Backup: {backup}")
        write_atomic(rc, desired, mode)
    if args.action == "uninstall" and hook.exists():
        hook.unlink()
    print(f"{args.action}: {rc}")
    print("Running Codex processes were not changed. Applies on the next full app start.")


if __name__ == "__main__":
    main()
