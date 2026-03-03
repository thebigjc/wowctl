# Contributing to wowctl

Thank you for your interest in contributing to wowctl! This document provides guidelines for contributing to the project.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/yourusername/wowctl.git`
3. Create a feature branch: `git checkout -b feature/your-feature-name`
4. Make your changes
5. Test your changes
6. Commit your changes: `git commit -m "Add your feature"`
7. Push to your fork: `git push origin feature/your-feature-name`
8. Open a pull request

## Development Setup

### Prerequisites

- Rust 1.85 or later (required for edition 2024)
- A CurseForge API key for testing (get one at https://console.curseforge.com/)

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test
```

### Running Locally

```bash
cargo run -- <command>
```

For example:
```bash
cargo run -- config show
cargo run -- --verbose search "deadly boss mods"
```

## Code Style

- Follow standard Rust formatting (use `cargo fmt`)
- Run `cargo clippy` and address any warnings
- Add documentation comments for public APIs
- Keep functions focused and concise
- Use meaningful variable names

## Testing

- Add tests for new functionality
- Ensure existing tests pass
- Test on both macOS and Windows if possible
- Test error cases and edge conditions

## Pull Request Guidelines

- Provide a clear description of what your PR does
- Reference any related issues
- Include test results if applicable
- Keep PRs focused on a single feature or fix
- Update documentation if needed

## Areas for Contribution

### High Priority

- Additional addon sources (WoWInterface, Wago, GitHub releases)
- Improved color control (fix --no-color flag)
- Comprehensive test suite
- Windows testing and bug fixes

### Medium Priority

- `adopt` command implementation (Phase 2)
- Version pinning feature
- Backup/restore of SavedVariables
- Export/import addon lists

### Low Priority

- JSON output mode for scripting
- WoW Classic support
- Disk space checking before downloads
- Progress bars for large downloads

## Questions?

Feel free to open an issue for any questions or discussions about contributing.

## License

By contributing to wowctl, you agree that your contributions will be licensed under the MIT License.
