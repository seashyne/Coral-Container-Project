# Coral Desktop

Cross-platform desktop GUI for the [Coral Container Runtime](../coral_repo/README.md), built with **Tauri v2** (Rust backend + vanilla JS frontend, no JS bundler).

Manages containers, pod sandboxes, and images over **CRI v1 gRPC** — the same API Kubernetes uses.

## Connection modes

| Mode | What it does | Platform |
| :--- | :--- | :--- |
| **Local** | Spawns `coral-daemon` on this machine and attaches | Linux |
| **WSL2** | Spawns `coral-daemon` inside a WSL2 distro via `wsl.exe`; reaches it through localhost forwarding | Windows |
| **Remote** | Attaches to an already-running daemon at `host:port` | Both |

If a daemon is already listening on the configured endpoint, the app attaches instead of spawning a new one. A locally spawned daemon is terminated when the app exits; a WSL2 daemon keeps running as a service.

Settings persist to the app config dir (`~/.config/com.coral.desktop/settings.json` on Linux, `%APPDATA%\com.coral.desktop\settings.json` on Windows).

## Development

### Linux prerequisites (Debian/Ubuntu)

```bash
sudo apt install -y pkg-config libwebkit2gtk-4.1-dev libssl-dev libxdo-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf
```

### Run

```bash
npm install
npm run dev
```

`npm run dev` first assembles the frontend shell: `src/index.html` is generated from
`src/index.template.html` + `src/partials/*.html` by `node build-html.mjs` (shared
assembler: `../tools/html-assembler.mjs`). Edit the partials, never the generated file —
it is git-ignored and rebuilt on every `dev` / `build` / `tauri` run. `cargo build` inside
`src-tauri` regenerates it too, so direct `cargo tauri build` still works.

In the app: **Settings** → mode `Local`, set:
- `coral-daemon binary`: `../coral_repo/target/debug/coral-daemon` (absolute path recommended)
- `coral CLI binary`: `../coral_repo/target/debug/coral`
- `Default rootfs`: `../coral_repo/rootfs`

Then **Save & Connect**. Run a container with image `alpine:latest`, command `/bin/sh`, args `-c "echo hello"`.

### Windows development

- Install [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (evergreen bootstrapper is bundled in the installer) and Rust MSVC toolchain.
- `npm install && npm run dev` builds the native Windows GUI.
- The runtime itself cannot run on Windows: use **WSL2 mode**. Inside your distro:

```bash
# in WSL2
cd coral_repo
./scripts/setup-rootfs.sh
cargo build --release --workspace
```

Then in the app set mode `WSL2` with Linux paths, e.g.
`/home/<you>/coral_repo/target/release/coral-daemon`, and Connect.

### Build installers

```bash
npm run build
```

- Linux: `.deb`, `.rpm`, `.AppImage` in `src-tauri/target/release/bundle/`
- Windows: `.msi` (WiX) and NSIS `.exe` in the same place

CI (`.github/workflows/desktop.yml`) builds both platforms on every push.

## Architecture

```text
desktop/
├── build-html.mjs          # Assembles src/index.html from the template + partials
├── src/
│   ├── index.template.html  # HTML shell template (include directives)
│   ├── index.html           # GENERATED — rebuilt by `npm run build:html` (git-ignored)
│   ├── partials/            # HTML shell sections (head, topbar, sidebar, views, dialogs, …)
│   ├── styles/
│   │   ├── main.css           # @layer tokens, reset, base, layout, components, views
│   │   ├── tokens/            # 3-tier CSS Design Tokens (primitives, semantic, components)
│   │   ├── base/              # Scoped element reset & layout utilities
│   │   ├── layout/            # Shell grid, topbar, sidebar rail, responsive rules
│   │   ├── components/        # Buttons, forms, tables, cards, badges, dialogs, drawer, terminal
│   │   └── views/             # View-specific layout styles
│   └── scripts/
│       ├── core/              # theme-init.js, state.js, theme.js, i18n.js
│       ├── locales/           # Bilingual EN/TH dictionaries (en/, th/)
│       ├── data/              # icons.js, catalog/
│       ├── features/          # connection, containers/, terminal/, images, volumes, pods, nextgen, settings
│       └── app/               # Event bindings & startup bootstrap (app/)
└── src-tauri/src/
    ├── lib.rs                 # Tauri builder, command registration, daemon cleanup on exit
    ├── settings.rs            # persisted connection settings (local / wsl2 / remote)
    ├── daemon.rs              # daemon lifecycle: probe → spawn (local/WSL2) → wait ready
    └── cri/                   # CRI v1 gRPC client commands:
        ├── mod.rs             # connection state & settings
        ├── containers.rs      # container lifecycle, logs, metrics, diff/commit, exec
        ├── pods.rs            # pod sandboxes
        ├── images.rs          # pull, build, tag, save/load, push
        ├── compose.rs         # compose up/down
        ├── system.rs          # df, prune, volumes, networks, registry login/logout
        ├── pty.rs             # full-duplex streaming PTY terminal (ExecStream gRPC)
        └── nextgen.rs         # v2.0 KVM MicroVM probe, WASI Wasm & AI Sandbox
```

The backend depends only on `coral-api` (pure prost/tonic bindings), so it compiles on Windows even though the runtime crates are Linux-only.