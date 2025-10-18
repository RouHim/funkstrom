# AGENTS.md - Funkstrom Coding Guidelines

## Color Schema - Deep Sea Radio

Official color palette for all visual assets:

- **Primary Gradient**: `#0891b2` (Cyan) → `#1e40af` (Deep Blue)
- **Background**: `#0c1821` (Deep ocean)
- **Accent**: `#14b8a6` (Teal)
- **Secondary**: `#67e8f9` (Light cyan)
- **Text**: `#cffafe` (Ice cyan)

Vibe: Oceanic, calm, professional with mysterious depth

## Build/Test/Lint Commands

- `cargo build` - Build the project
- `cargo run -- --config config.toml` - Run with config file
- `cargo test` - Run all Rust unit tests
- `cargo test <test_name>` - Run specific test
- `cargo clippy` - Lint code
- `cargo fmt` - Format code
- `./e2e/test.sh` - Run E2E tests (requires running server)

## Code Style Guidelines

- Breaking changes are allowed because the app is not released yet; no migration is needed when implementing new
  features or changes.
- `cargo run` hangs until the process gets killed, thus always start the application in the background using `nohup`
- **Error Handling**: Use `Result<T, Box<dyn std::error::Error>>` for fallible functions
- **Async**: Use tokio runtime, prefer async/await over blocking operations
- **Channels**: Use crossbeam-channel for thread communication, tokio channels for async
- **Logging**: Use log crate with env_logger, structured logging with context
- **Naming**: snake_case for variables/functions, PascalCase for structs/enums
- **Logging**: Use the `log` crate with `env_logger` for logging.

## Code Quality Principles

### Modularity

- Structure code into small, focused rust files without using rust modules
- Each file should encapsulate a single responsibility or closely related functionalities.
- Promote reusability and ease of testing by isolating components.

### SOLID Principles

- Follow the SOLID object-oriented design principles to ensure maintainable and extensible code.
- Emphasize single responsibility, open-closed, Liskov substitution, interface segregation, and dependency inversion
  where applicable.

### Clean Code

- Write clear, readable, and straightforward code.
- Use descriptive names and avoid clever tricks or shortcuts that hinder comprehensibility.
- Keep functions and files focused and concise.
- YAGNI - You Aren't Gonna Need It: Avoid adding functionality until it is necessary.
- Don't write unused code for future features.

## Dependency Management

- Avoid introducing additional dependencies unless absolutely necessary.
- Prefer standard Rust libraries and built-in features to minimize external package usage.
- Evaluate trade-offs before adding any third-party crate.

## Formatting and Linting

- Always run code formatters (`cargo fmt`) and linters (`cargo clippy`) before committing.
- Maintain consistent code style across the project to improve readability and reduce friction in reviews.

## Testing Practices

### Test-Driven Development (TDD)

- When it makes sense, write tests before coding the functionality.
- Use tests to drive design decisions and ensure robust feature implementation.

### Behavior-Driven Development (BDD)

- Write tests in a BDD style, focusing on the expected behavior and outcomes.
- Structure tests to clearly state scenarios, actions, and expected results to improve communication and documentation.

### E2E Testing

- End-to-end tests are located in `e2e/` directory
- Tests use bash scripts with curl and jq
- Run `./e2e/test.sh` to execute basic test suite (6 tests total)
- Tests cover HTTP endpoints, Icecast headers, streaming, and buffer status

## Available Tools

- If needed use Context7 and Online Search to clarify dependency APIs or implementation details or example usages
- Do NOT use sudo, ask me for executing commands if needed!
- Use `pkill funkstrom` to kill the application if it is already running

## Warnings

- `cargo run` hangs until the process gets killed, thus always start the application in the background using `nohup`
