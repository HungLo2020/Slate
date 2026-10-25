# Development utilities

Everything in this directory supports building, testing, publishing and
diagnosing Slate. None of it is installed with the editor. Agents and
contributors should start with `Build.sh`, which runs the full local gate.

## Layout

| Path | Purpose |
| --- | --- |
| `Build.sh` | Full local gate: dependency policy, Python tests, `cargo fmt --check`, Clippy, the Cargo test suite, the amd64 package build and the DEB smoke test. |
| `InstallDependencies.sh` | Installs the distro packages needed to build and test on amd64 Debian/Ubuntu. |
| `apt-dependencies.txt` | Package list read by `InstallDependencies.sh`. |
| `BuildScripts/` | Packaging scripts. `build-linux-amd64.sh` builds the `.deb` in `builds/` from the `[workspace.package]` version. |
| `PublishMattOSPackage.py` | Validates and publishes the DEB to MattPackages. `doctor`, `publish --dry-run` and `publish` subcommands. |
| `RunGui.py` | Builds the release binaries and launches the GUI. |
| `RunTui.py` | Builds the release binaries and launches the TUI in a new terminal. |
| `dependency-policy.py` | Enforces the crate dependency policy described in the README. |
| `SmokeTests/` | Black-box tests that drive real executables. Grouped by surface. |

## Smoke tests

| Directory | Coverage |
| --- | --- |
| `SmokeTests/Terminal/` | PTY-driven checks: editing and saving (`tui-smoke.py`), startup modes, tab closing, crash recovery and idle CPU. CI also runs the recovery check against `slate-gui`. |
| `SmokeTests/Gui/` | Qt Quick checks: offscreen feature/settings runs, image previews, geometry and large fonts, layout styles, performance, termination, clipboard, single instance, editor wait and Wayland/AT-SPI desktop integration. |
| `SmokeTests/Packaging/` | `install-smoke.py` exercises `scripts/install.sh` in an isolated prefix; `deb-smoke.py` validates and runs a built DEB. |

Run a smoke test with the executable as its argument, for example:

```bash
cargo build --release
python3 DevUtils/SmokeTests/Terminal/tui-smoke.py target/release/slate
python3 DevUtils/SmokeTests/Packaging/install-smoke.py target/release/slate
```

`scripts/install.sh` stays outside this directory: it is the user-facing
installer documented in the README, not a development utility.
