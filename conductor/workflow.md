# Project Workflow

## Guiding Principles

1. **The Plan is the Source of Truth:** All work must be tracked in `plan.md`
2. **The Tech Stack is Deliberate:** Changes to the tech stack must be documented in `tech-stack.md` _before_ implementation
3. **Test-Driven Development:** Write unit tests before implementing functionality
4. **High Code Coverage:** Aim for >80% code coverage for all modules
5. **User Experience First:** Every decision should prioritize user experience
6. **Non-Interactive & CI-Aware:** Prefer non-interactive commands. Use `CI=true` for watch-mode tools (tests, linters) to ensure single execution.

## Task Workflow

All tasks follow a strict lifecycle:

### Standard Task Workflow

1. **Select Task:** Choose the next available task from `plan.md` in sequential order

2. **Mark In Progress:** Before beginning work, edit `plan.md` and change the task from `[ ]` to `[~]`

3. **Write Failing Tests (Red Phase):**
   - Create a new test file for the feature or bug fix.
   - Write one or more unit tests that clearly define the expected behavior and acceptance criteria for the task.
   - **CRITICAL:** Run the tests and confirm that they fail as expected. This is the "Red" phase of TDD. Do not proceed until you have failing tests.

4. **Implement to Pass Tests (Green Phase):**
   - Write the minimum amount of application code necessary to make the failing tests pass.
   - Run the test suite again and confirm that all tests now pass. This is the "Green" phase.

5. **Refactor (Optional but Recommended):**
   - With the safety of passing tests, refactor the implementation code and the test code to improve clarity, remove duplication, and enhance performance without changing the external behavior.
   - Rerun tests to ensure they still pass after refactoring.

6. **Verify Coverage:** Run coverage reports using the project's chosen tools.

   ```bash
   cargo tarpaulin --out Html --output-dir coverage/
   ```

   Target: >80% coverage for new code.

7. **Document Deviations:** If implementation differs from tech stack:
   - **STOP** implementation
   - Update `tech-stack.md` with new design
   - Add dated note explaining the change
   - Resume implementation

8. **Commit Code Changes:**
   - Stage all code changes related to the task.
   - Propose a clear, concise commit message e.g, `feat(fe-network): Add libp2p Kademlia DHT peer discovery`.
   - Perform the commit.

9. **Attach Task Summary with Git Notes:**
   - **Step 9.1: Get Commit Hash:** Obtain the hash of the _just-completed commit_ (`git log -1 --format="%H"`).
   - **Step 9.2: Draft Note Content:** Create a detailed summary for the completed task. This should include the task name, a summary of changes, a list of all created/modified files, and the core "why" for the change.
   - **Step 9.3: Attach Note:** Use the `git notes` command to attach the summary to the commit.
     ```bash
     git notes add -m "<note content>" <commit_hash>
     ```

10. **Get and Record Task Commit SHA:**
    - **Step 10.1: Update Plan:** Read `plan.md`, find the line for the completed task, update its status from `[~]` to `[x]`, and append the first 7 characters of the commit hash.
    - **Step 10.2: Write Plan:** Write the updated content back to `plan.md`.

11. **Commit Plan Update:**
    - Stage the modified `plan.md` file.
    - Commit with a descriptive message (e.g., `conductor(plan): Mark task 'Add Kademlia DHT' as complete`).

### Phase Completion Verification and Checkpointing Protocol

**Trigger:** This protocol is executed immediately after a task is completed that also concludes a phase in `plan.md`.

1.  **Announce Protocol Start:** Inform the user that the phase is complete and the verification and checkpointing protocol has begun.

2.  **Ensure Test Coverage for Phase Changes:**
    - **Step 2.1: Determine Phase Scope:** Find the starting point from `plan.md` (previous phase checkpoint SHA). If none, scope is all changes since first commit.
    - **Step 2.2: List Changed Files:** Execute `git diff --name-only <previous_checkpoint_sha> HEAD`.
    - **Step 2.3: Verify and Create Tests:** For each code file in the list, verify a corresponding test file exists. If missing, create one matching the project's naming conventions.

3.  **Execute Automated Tests with Proactive Debugging:**
    - Announce the exact command before running it.
    - **Command:** `cargo test 2>&1`
    - If tests fail, attempt to fix a maximum of **two times**. If still failing after two attempts, stop and ask the user for guidance.

4.  **Propose a Detailed Manual Verification Plan:**
    - Analyze `product.md` and `plan.md` to determine user-facing goals of the completed phase.
    - Generate a step-by-step verification plan with specific expected outcomes.

5.  **Await Explicit User Feedback:**
    - Ask: "Does this meet your expectations? Please confirm with yes or provide feedback."
    - **PAUSE** and await response. Do not proceed without explicit confirmation.

6.  **Create Checkpoint Commit:**
    - Stage all changes and commit: `conductor(checkpoint): Checkpoint end of Phase X`.

7.  **Attach Auditable Verification Report using Git Notes:**
    - Draft a verification report (test command, manual steps, user confirmation).
    - Attach via `git notes add -m "<report>" <commit_hash>`.

8.  **Get and Record Phase Checkpoint SHA:**
    - Obtain the checkpoint commit hash.
    - Update `plan.md` phase heading with `[checkpoint: <sha>]`.
    - Write updated `plan.md`.

9.  **Commit Plan Update:**
    - `conductor(plan): Mark phase '<PHASE NAME>' as complete`

10. **Announce Completion.**

### Quality Gates

Before marking any task complete, verify:

- [ ] All tests pass (`cargo test`)
- [ ] Code coverage meets requirements (>80% via `cargo tarpaulin`)
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All public functions have doc comments (`///`)
- [ ] No `unwrap()` or `expect()` in production code paths
- [ ] All gossip messages carry ed25519 signatures (enforced by type system)
- [ ] No `block_on()` calls inside Bevy systems
- [ ] RBAC checks only in SurrealDB layer, not in Bevy systems
- [ ] Security-relevant events logged via `tracing`
- [ ] No hardcoded secrets or private key material in code

## Development Commands

### Setup

```bash
# Install Rust stable toolchain
rustup toolchain install stable
rustup component add rustfmt clippy

# Install cargo-tarpaulin for coverage
cargo install cargo-tarpaulin

# Build the project
cargo build
```

### Daily Development

```bash
# Run all tests
cargo test

# Run with logging
RUST_LOG=debug cargo run

# Format check
cargo fmt --check

# Lint
cargo clippy -- -D warnings

# Coverage report
cargo tarpaulin --out Html --output-dir coverage/
```

### Before Committing

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

## Testing Requirements

### Unit Testing

- Every module must have corresponding tests in `#[cfg(test)]` blocks.
- Use `tokio::test` for async functions in `fe-network` and `fe-database`.
- Mock external peers using in-process channels; never require a live network for unit tests.
- Test both success and failure cases for all auth, crypto, and RBAC functions.

### Integration Testing

- Integration tests in `tests/` directory at the crate root.
- Test the full session handshake flow (connect → JWT issue → role assign → verify → revoke).
- Test RBAC enforcement at the SurrealDB layer (public cannot write, custom role can within scope, admin has full access).
- Test WebView IPC command dispatch for all `BrowserCommand` variants.

### Security Testing

- Every signature verification function must have a test that passes a tampered message and asserts rejection.
- WebView URL denylist must have tests for localhost, 127.0.0.1, and each RFC 1918 range.
- JWT expiry and revocation propagation must have integration tests with synthetic clock advancement.

## Commit Guidelines

### Message Format

```
<type>(<scope>): <description>
```

### Types

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `style`: Formatting only
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `test`: Adding or updating tests
- `chore`: Maintenance tasks
- `conductor`: Conductor plan/checkpoint updates

### Scopes (match module names)

`fe-runtime`, `fe-identity`, `fe-database`, `fe-network`, `fe-world`, `fe-renderer`, `fe-webview`, `fe-auth`, `fe-ui`

### Examples

```bash
git commit -m "feat(fe-network): Add libp2p Kademlia DHT peer discovery"
git commit -m "feat(fe-auth): Implement signed JWT session handshake"
git commit -m "fix(fe-webview): Block RFC 1918 addresses in navigation handler"
git commit -m "test(fe-identity): Add verify_strict failure case tests"
```

## Definition of Done

A task is complete when:

1. All code implemented to specification
2. Unit tests written and passing (TDD: red → green → refactor)
3. Code coverage >80% for new code
4. `cargo fmt` and `cargo clippy -- -D warnings` both pass
5. All public functions have `///` doc comments
6. Security rules from `general.md` verified (signatures, RBAC placement, no unsafe in auth paths)
7. Implementation notes appended to `plan.md`
8. Changes committed with proper message format
9. Git note with task summary attached to the commit
