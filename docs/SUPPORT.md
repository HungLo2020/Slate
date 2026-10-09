# Supported environments and release policy

Slate supports Linux on amd64. The terminal executable does not link Qt. The graphical executable uses Qt 6.4 or later and Qt Quick Controls; it works without KDE frameworks. KDE desktop integration is additionally tested where installed. Other architectures and operating systems are not release targets.

Source checks cover Ubuntu 24.04, the minimum Rust version (1.89), and GUI/runtime tests on Debian trixie and Fedora. Release builds use Rust 1.98 as declared in `rust-toolchain.toml`. Use `cargo +1.89.0 check --locked --package slate --all-targets` to verify the minimum compiler. Run `DevUtils/Build.sh` for the full local package gate.

The amd64 DEB is built in Ubuntu 26.04. It requires the library versions recorded in its generated dependency metadata; it is not a promise of binary compatibility with older Debian/Ubuntu releases. Build from source on older supported test distributions. Install with `--no-install-recommends` for a terminal-only machine. The separate `Build Linux amd64 DEB` workflow builds an artifact; it does not publish it.

Real publication requires a clean `main`, matching `origin/main`, a successful latest **Slate checks** push run for that exact commit, and a DEB whose embedded source commit and clean-tree marker match. Publication also refuses an existing package version. `--dry-run` exercises artifact validation without uploading; it does not establish release readiness.

Recovery schema 2 stores checkpoint metadata and content-addressed buffer payloads in the private state directory. Schema 1 is readable. Buffers are limited to 1 GiB each; metadata is limited to 128 MiB. A damaged payload is excluded while other buffers are salvaged. Watch the persistent recovery warning; it means the last checkpoint is not current. Switching a terminal workspace drains pending work before releasing its recovery lock. GUI Open Folder opens another window so unsaved work remains available.

`Support report` opens a read-only report containing the version, platform, configuration/state paths, pending-save and recovery state, and up to 100 bounded recent error messages. Persistent error history is private and capped; document contents are not collected explicitly. Helper errors and paths can contain private information: review before sharing. Search/replacement history persists only with `history_log = true` or `--historylog`. Recent files and project lists remain local.

Report reproducible defects with the support report, exact steps, expected/actual behavior, frontend, display backend, and a minimal non-private example. Include the commit when testing an unreleased build. A passing workflow verifies its tested environments and scenarios; it does not establish equivalence with all features of Kate, VS Code or nano.

A source license has not yet been selected by the project owner. No license grant is implied by the public repository or its build artifacts. Licensing remains a release decision.
