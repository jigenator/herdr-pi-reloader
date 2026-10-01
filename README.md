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
- **Reset all Pi** restarts eligible panes **one at a time**, waits for each recorded session to become ready, and briefly visits panes that were previously reported `idle` to acknowledge startup badges.
- Shows reset progress in Herdr's top-right bar, then returns to the original pane/popup with completed, skipped, and failed counts.
- Rechecks each pane before quitting; busy, non-Pi, and changed sessions are not reset. Concurrent resets on the same server are rejected.

## Requirements

- Herdr 0.9.0 or newer
- The [Pi coding agent](https://github.com/badlogic/pi-mono) available on `PATH`
- The Herdr Pi integration
- Rust 1.89 or newer with Cargo (required to build the plugin during installation)
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
**Open Pi Reloader**. No separate reset/reloader shortcut is needed.

### Reset progress bar — configure before using Reset

Add a command entry to your existing `[ui].tab_bar_right` array in Herdr's
`config.toml`, preserving the other entries:

```toml
{ type = "command", command = "\"/absolute/plugin/root/target/release/herdr-pi-reloader\" status", interval_seconds = 1, timeout_seconds = 1 },
```

Replace `/absolute/plugin/root` with `plugin_root` from
`herdr plugin list --plugin herdr-pi-reloader.pi-reloader --json`, then run
`herdr server reload-config`. This reloads Herdr configuration, not Pi sessions.
The installed path stays stable across plugin upgrades.

During reset the bar shows, for example:

```text
DON'T TYPE | Pi [####----] 2/4 w3:p1 waiting
```

The indicator disappears on completion. `status` only reads local progress; it
never queries, focuses, or resets panes. Progress and an OS reset lock live next
to the active `HERDR_SOCKET_PATH`, independently for each server. A crashed reset
cannot leave a permanently active indicator or lock. The original pane is
restored on normal completion and handled errors; forced process termination
cannot guarantee restoration.

**Do not type, click, switch panes, or leave the Herdr window until the summary
returns.** The popup belongs to its original tab and disappears while visiting
other tabs; **it does not block input there**. Herdr's public pane-focus API moves
all attached clients on the same server, so use this with one active client/view.
Visiting a split tab also acknowledges other visible panes in that tab. Previously
`done` reset candidates are not deliberately visited, but exact per-client
acknowledgement preservation is not possible through Herdr's API.

A visit waits one second for a focused local client to render. Herdr has no public
client acknowledgement receipt: `Visited` means focus succeeded, not a verified
badge change in every client. Unfocused or slow remote clients may still show
`done`. Restart success requires the original session path, an idle/done report,
and a foreground Pi process—not just successful command submission.

**Live testing is manual.** Reset affects every eligible Pi session, including an
assistant's own idle session. Finish automated work before testing from the popup.
The CLI `reset` entrypoint also moves focus and requires `HERDR_SOCKET_PATH`;
`reload` retains its existing behavior.

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
process identity, non-Pi rejection, refreshed busy/session guards, process errors,
quoted session resume, shell/startup readiness, focus restoration, progress isolation,
and crash-safe locking.

An opt-in integration test starts a real Herdr server and native TUI in a private
PTY with a temporary HOME/socket and a fake `pi` executable first on PATH:

```sh
HERDR_RELOADER_ISOLATED=1 python3 -m unittest discover -s tests -p test_isolated_herdr.py -v
```

It checks sequential restarts, session preservation, busy skips, visible progress,
client idle badges, and return to the original popup. It requires Node and Bash;
no real Pi sessions are started or reset. Logs/screens remain in the printed
`/tmp/pir-ui-*` evidence directory. `RELOADER_BINARY` selects another build.

## Contributing

Contributions are highly welcome. If you have an idea for improving the workflow, user experience, reliability, or platform support, feel free to [open an issue](https://github.com/jigenator/herdr-pi-reloader/issues) or submit a pull request.

Please report any bugs or unexpected behavior through GitHub Issues. When possible, include your Herdr and plugin versions, operating system, reproduction steps, expected and actual behavior, and relevant plugin logs.

## Trust and security

Herdr plugins run as the current user and are not sandboxed. Review [`herdr-plugin.toml`](./herdr-plugin.toml) and the source before installing.
