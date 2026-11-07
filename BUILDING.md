# Building 3db from Source

3db is a Windows desktop application built with Tauri 2, Rust, React, TypeScript, Vite, and SQLite.

## Requirements

On Windows, install:

- Git
- Node.js and npm
- Rust with the MSVC toolchain
- Microsoft C++ Build Tools and Windows SDK
- Microsoft Edge WebView2 Runtime

For full platform requirements, consult the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

## Clone the Repository

```powershell
git clone https://github.com/FabimachtSachen/3db.git
cd 3db
```

## Install Dependencies

```powershell
npm ci
```

## Build the Frontend

```powershell
npm run build
```

This runs TypeScript checks and creates the Vite production build.

## Run Rust Tests

```powershell
cd src-tauri
cargo test --locked
cd ..
```

## Build the Windows Installer

```powershell
.\node_modules\.bin\tauri.cmd build --bundles nsis
```

The Tauri build automatically executes the configured frontend build command.

The NSIS installer is normally generated in:

`src-tauri/target/release/bundle/nsis/`

If `CARGO_TARGET_DIR` is configured, Cargo uses that alternative directory instead.

## Development

Refer to `package.json` for available npm scripts and `src-tauri/tauri.conf.json` for the Tauri configuration.

## Data Safety

- Use test projects rather than personal production data.
- Back up existing 3db data before testing migrations.
- Do not commit user databases, secrets, or build artifacts.
- Do not use development builds with production data without a backup.

## Release Information

See `CHANGELOG.md` and `docs/releases/v1.0.0.md` for the initial release information.

The official Windows installer is distributed through GitHub Releases.
