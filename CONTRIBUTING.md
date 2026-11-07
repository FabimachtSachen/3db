# Contributing to 3db

Thank you for your interest in contributing to 3db!

3db is an open-source desktop application for managing personal
3D printing projects.

## Getting Started

1. Fork the repository.
2. Create a dedicated branch for your change.
3. Keep changes focused and reviewable.
4. Test your changes locally.
5. Open a pull request describing your changes.

## Development

The project uses:

- Tauri and Rust
- React and TypeScript
- Vite
- SQLite

Dependencies and build instructions are documented in the repository.

## Contribution Guidelines

- Preserve compatibility with existing project data.
- Avoid destructive database or filesystem operations.
- Include tests for changes affecting imports, migrations, or recovery.
- Do not commit personal project libraries or generated build files.
- Do not introduce unrelated features into bug-fix pull requests.
- Keep the application's public branding consistent with 3db.

## Pull Requests

Please explain:

- What changed
- Why the change is necessary
- How it was tested
- Whether existing user data could be affected

## Bug Reports

When reporting a bug, include the application version, Windows
version, steps to reproduce, and relevant error messages.

Remove personal paths, private project data, and credentials
before attaching logs.

## License

By submitting a contribution, you agree that your contribution
is made available under the project's MIT License.
