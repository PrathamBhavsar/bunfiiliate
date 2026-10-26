# Contributing to bunfiiliate

Thank you for your interest in contributing to bunfiiliate. Please read this guide thoroughly before submitting changes.

---

## Clean-Room Engineering Policy

bunfiiliate is an original, clean-room open-source project. Contributors must strictly observe the following rules:

- **No proprietary code or schemas**: Never copy, consult, or adapt code, database schemas, API specs, or copy from proprietary or closed-source affiliate tracking platforms.
- **Independent design**: All implementations must be derived purely from open public RFCs, standard domain documentation, or specifications in [`docs/PLAN.md`](docs/PLAN.md) and [`brain/`](brain/).

---

## Git Flow & Branching Model

We follow a strict, disciplined three-tier git flow:

```text
prod  <--- (release gate only) ---  staging  <--- (merge --no-ff) ---  feat/*
```

1. **Feature Branches (`feat/*`)**:
   - All work happens in isolated feature branches branched off `staging`.
   - Work only on the designated branch in your worktree.
   - Keep diffs small, focused, and free of speculative abstractions.

2. **Integration (`staging`)**:
   - When a task or feature branch passes its local acceptance gate and `make ci`, it is merged into `staging` using `git merge --no-ff`.
   - Never commit directly to `staging`.

3. **Production (`prod`)**:
   - `staging` is merged into `prod` only at milestones after the full automated test suite, code reviews, and end-to-end verification gates pass.
   - Never commit directly to `prod`.

4. **No Remote Push During Development**:
   - Do not push branches or tags to remote repositories until the local final gate (`make ci`, open-code-review, and e2e) has completed successfully.

---

## Commit Message Style

Commit messages must be concise, descriptive, and written in the imperative mood. Do not use filler or narrative prose.

Format:

```text
<type>: <short summary>
```

Allowed types:

- `feat`: A new feature or capability.
- `fix`: A bug fix.
- `test`: Adding, updating, or refactoring tests.
- `docs`: Documentation updates.
- `refactor`: Code reorganization with no functional or API changes.
- `perf`: Performance optimization.
- `chore`: Toolchain, dependency, or configuration updates.

Examples:

```text
feat: add wal spool group commit
fix: resolve race in checkpoint segment deletion
test: add proptests for round-half-up commission math
docs: document tracker hot-path architecture
```

---

## Local Development & Prerequisites

### Rust Environment

- Toolchain: Rust 1.80+ (managed via `rustup`).
- On macOS arm64, ensure the rustup binary directory precedes system paths:

  ```bash
  export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
  ```

### PostgreSQL 16 Setup

bunfiiliate targets PostgreSQL 16. To ensure zero conflicts with local development environments running PostgreSQL on the default port:

- **Port 5433**: Local Postgres 16 runs on port `5433` (do **not** use port 5432).
- **UUIDv7 in Rust**: We generate UUIDv7 identifiers in Rust using the `uuid` crate (`v7` feature), never relying on PostgreSQL 18's native `uuidv7()`.
- **Dedicated cluster directory**: `~/.bunfiiliate/pg16`.

To start the local PostgreSQL 16 instance:

```bash
/opt/homebrew/opt/postgresql@16/bin/pg_ctl -D ~/.bunfiiliate/pg16 -o "-p 5433 -k /tmp" start
```

To stop the instance when done:

```bash
/opt/homebrew/opt/postgresql@16/bin/pg_ctl -D ~/.bunfiiliate/pg16 stop
```

Set the database connection string (configured with trust authentication locally):

```bash
export DATABASE_URL=postgres://pratham@localhost:5433/postgres
```

Run database migrations:

```bash
cargo run -p api -- bootstrap \
  --email admin@example.com \
  --password "SuperSecretPassword123!" \
  --workspace "Default"
```

---

## Continuous Integration (`make ci`)

All changes must pass `make ci` locally before any merge. `make ci` mirrors the CI pipeline and runs:

1. `cargo fmt --check`: Enforces Rust formatting standards.
2. `cargo clippy --workspace -- -D warnings`: Fails on any Clippy warnings across all targets.
3. `cargo test --workspace`: Executes all unit, integration, and property-based tests.
4. `cargo deny check`: Audits dependencies for license compliance and known advisories.

Run all checks with a single command:

```bash
make ci
```

---

## Knowledge Bundle Upkeep (`brain/`)

bunfiiliate uses an on-disk knowledge bundle located in `brain/` to maintain architectural decisions, operational history, and active task states:

1. **Read `brain/index.md` first**: Always start by reading `brain/index.md` and navigate via linked concepts.
2. **Update Task Status**: When completing a task, update its row in [`brain/tasks/index.md`](brain/tasks/index.md).
3. **Log Changes**: Prepend a concise entry at the very **top** of [`brain/log.md`](brain/log.md) summarizing the creation/modification, test output, and updated concepts.
4. **Record Lessons & Anti-Patterns**: Document any newly discovered pitfalls, traps, or architectural invariants in [`brain/wisdom/`](brain/wisdom/).
5. **Strict Validation**: Validate the integrity of the knowledge bundle using the OKF validator:

   ```bash
   python3 ~/.claude/skills/okf/scripts/validate.py brain --strict
   ```

   All checks must pass with `0 error(s)` and `0 lint(s)`.

---

## Documentation Linting

Markdown documentation is validated using `markdownlint-cli2`:

```bash
npx markdownlint-cli2 "**/*.md" "#node_modules" "#brain"
```

Ensure headings, code blocks, lists, and tables adhere to Markdown standards prior to submitting changes.
