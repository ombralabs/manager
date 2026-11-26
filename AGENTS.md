# Agent & Contributor Guidelines

## Branch and Pull Request Workflow

- **Never commit or push directly to `main`.** The `main` branch is protected against direct pushes, force pushes, and deletions (including for repository admins). All changes must enter `main` through a Pull Request.
- **One branch per task.** Before making the first edit, create a dedicated branch from the latest `main` using a clear name (for example, `feat/<short-name>`, `fix/<short-name>`, `chore/<short-name>`, or `task/<short-name>`). Continue work on the same task in its existing branch.
- **Do not mix unrelated tasks.** If the working copy already has unfinished changes from another task, use a separate `git worktree`. Never revert, move, or include unrelated changes in the current task's commits.
- **Merge only through PR after CI passes.** Open a Pull Request targeting `main`, ensure required CI checks (`Vue typecheck, tests and build` and `Rust tests and clippy`) are green and all review conversations are resolved before merging.

## Verification Before Opening or Updating a PR

Run the relevant checks locally before pushing:

- **Frontend / TypeScript / release scripts:**
  ```sh
  npm run check:web && npm run build
  node --test tools/release/*.node-test.mjs
  ```
- **Rust (`src-tauri`):**
  ```sh
  cargo test --locked --manifest-path src-tauri/Cargo.toml
  cargo clippy --locked --all-targets --manifest-path src-tauri/Cargo.toml -- -D warnings
  ```

## Public Repository & License Boundaries

- This repository (including `vendor/ombra-engine/crates/ombra-humanize`) is public and licensed under **`GPL-3.0-only`**.
- Ombra Manager must remain 100% local and autonomous — do not introduce mandatory cloud account sign-in, external licensing gates, or proprietary components.
- Never commit secrets, credentials, proprietary engine code, or private local filesystem paths/internal hostnames.
