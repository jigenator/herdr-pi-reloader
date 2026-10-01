# Herdr Pi Reloader

A small [Herdr](https://herdr.dev/) plugin for safely reloading or restarting Pi agent sessions from a popup TUI without changing the tiled tab layout.

The plugin only operates on Pi agents that Herdr reports as `idle` or `done`. Busy agents are skipped.

This fork of [anrunt/herdr-pi-reloader](https://github.com/anrunt/herdr-pi-reloader)
fixes Herdr 0.9 process detection (`name: "node", argv0: "pi"`) and reports non-Pi
skips instead of misleading zero counts. The plugin and action IDs are unchanged.

## Preview
![Herdr Pi Reloader popup with reset and reload actions](https://github.com/user-attachments/assets/09346711-c37e-4ad4-a422-0e05b0a3ce90)

## Features

- **Reload all Pi** sends `/reload` to every eligible Pi pane.
- **Reset all Pi** exits each eligible Pi process and resumes its recorded session with `pi --session`.
- Provides a session-modal popup TUI with a summary of completed, skipped, and failed operations.

## Requirements

- Herdr 0.7.4 or newer
- The [Pi coding agent](https://github.com/badlogic/pi-mono) available on `PATH`
- The Herdr Pi integration
- Rust 1.85 or newer with Cargo (required to build the plugin during installation)
- Linux or macOS

## Installation

Install the Herdr integration for Pi first:

```sh
herdr integration install pi
```

If the upstream plugin is already installed, uninstall it first:

```sh
herdr plugin uninstall anrunt/herdr-pi-reloader
```

Then install this fork:

```sh
herdr plugin install jigenator/herdr-pi-reloader
```

Herdr shows the repository and build commands for review before installation. The plugin is compiled locally with Cargo and stored in Herdr's managed plugin directory.

Confirm that it is installed and enabled:

```sh
herdr plugin list
herdr plugin action list --plugin herdr-pi-reloader.pi-reloader
```

## Usage

Open the popup from the command line:

```sh
herdr plugin action invoke herdr-pi-reloader.pi-reloader.open
```

Inside the popup:

- `j` / `Down` — move down
- `k` / `Up` — move up
- `Enter` — run the selected operation
- `q`, `Esc`, or `Ctrl+C` — close (disabled while a reset is running)

With [Herdr Spotlight](https://github.com/jigenator/herdr-spotlight), search for
**Open Pi Reloader**. No separate reset/reloader shortcut or plugin configuration is needed.

**Live testing is manual.** Reset affects every eligible Pi session, including an
assistant's own idle session. Finish automated work before testing from the popup.

## Updating

Reinstall from the fork to replace its managed checkout with the latest version:

```sh
herdr plugin install jigenator/herdr-pi-reloader
```

Use `--ref` with a commit or tag to pin a specific revision.

## Uninstalling

```sh
herdr plugin uninstall jigenator/herdr-pi-reloader
```

## Local development

Herdr does not run manifest build commands for locally linked plugins, so build the binary first:

```sh
git clone https://github.com/jigenator/herdr-pi-reloader.git
cd herdr-pi-reloader
cargo build --release --locked
herdr plugin link .
```

Open the linked plugin:

```sh
herdr plugin action invoke herdr-pi-reloader.pi-reloader.open
```

Inspect plugin logs or remove the local link with:

```sh
herdr plugin log list --plugin herdr-pi-reloader.pi-reloader
herdr plugin unlink herdr-pi-reloader.pi-reloader
```

## Tests

```sh
cargo test --locked
cargo build --locked
python3 -m unittest discover -s tests -v
```

The Python checks use a fake Herdr executable and isolated environment. Reload/reset
commands are recorded, never executed against live panes. They cover current and legacy
process identity, non-Pi rejection, busy/session guards, process errors, and quoted session resume.

## Contributing

Contributions are highly welcome. If you have an idea for improving the workflow, user experience, reliability, or platform support, feel free to [open an issue](https://github.com/jigenator/herdr-pi-reloader/issues) or submit a pull request.

Please report any bugs or unexpected behavior through GitHub Issues. When possible, include your Herdr and plugin versions, operating system, reproduction steps, expected and actual behavior, and relevant plugin logs.

## Trust and security

Herdr plugins run as the current user and are not sandboxed. Review [`herdr-plugin.toml`](./herdr-plugin.toml) and the source before installing.
