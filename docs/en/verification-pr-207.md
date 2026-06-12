# Manual Verification Guide — PR #207 (Tab Completion Regression)

> Companion to https://github.com/AI-Shell-Team/aish/pull/207
>
> This guide explains how to verify the fixes for issue
> [#207](https://github.com/AI-Shell-Team/aish/issues/207) (Tab completion
> regression in v0.3.0) by hand on a real Linux/macOS terminal.

## What this PR fixes

| # | Symptom | Root cause | File |
|---|---|---|---|
| 1 | Multiple candidates sharing a prefix would not auto-complete | `query_completions` issued a new `__aish_complete` request without draining the control pipe; a stale `PromptReady{command_seq:null}` from `forward_readline_tab`'s `set -o emacs` / `set +o emacs; set +o vi` probes pre-emptively matched the new submission and broke the wait loop. | `crates/aish-pty/src/aish_completion.rs` |
| 2 | The screen could blank with stray output after a Tab | Same root cause plus leftover PTY echo state from the readline-tab probes. | (covered by fix 1) |
| 3 | Stale candidate list when only one match remained | A `PromptReady{null}` event could steal a backend submission registered with a seq. `CommandState::take_submission(None)` unconditionally took `active_submission`. | `crates/aish-pty/src/command_state.rs` |
| 4 | `cd Doc<TAB>` produced `cd ./Documents/` (extra `./` prefix) | Bash's `_aish_resolve_compreply` prepended `./` to bare filename completions. | `crates/aish-pty/src/bash_rc_wrapper.sh` |

## 0. Preparation

### 0.1 Check out the PR

```bash
git clone https://github.com/AI-Shell-Team/aish.git   # or use your fork
cd aish
git fetch origin pull/207/head:pr-207
git checkout pr-207

# Confirm you are on the PR commit
git log -1 --oneline
# expected: 37ff508 fix(pty): drain stale control-pipe events before tab completion query (#207)
```

### 0.2 Build the binary

```bash
rustup toolchain install 1.95.0 --profile minimal --component clippy --component rustfmt
cargo build --release
```

If you only want to verify (without the full release), `cargo run --bin aish` is fine.

### 0.3 Prepare a test directory

```bash
mkdir -p /tmp/aish-tab-test/Documents /tmp/aish-tab-test/Downloads
ls /tmp/aish-tab-test
# expected: Documents  Downloads
```

### 0.4 LLM API key (only if you exercise the AI features)

The shell needs a model + API key on first launch. Set a dummy key in `~/.config/aish/config.yaml` if you only want to test tab completion:

```yaml
model: openai/gpt-4o-mini
api_base: https://api.openai.com/v1
api_key: dummy
```

Tab completion does not need a real key. AI features (`;query`) will fail but completion still works.

---

## 1. Automated checks (run these first — fastest, most reliable)

```bash
cargo test -p aish-pty --lib command_state
```

Expected output (all 9 tests pass, including 2 new regressions):

```
test command_state::tests::test_stale_prompt_ready_with_null_seq_does_not_consume_backend_submission ... ok
test command_state::tests::test_prompt_ready_with_null_seq_still_matches_user_submission ... ok
test command_state::tests::test_register_and_complete_user_command ... ok
test command_state::tests::test_backend_command_with_seq ... ok
... (5 more)
test result: ok. 9 passed; 0 failed
```

The two new tests (`test_stale_prompt_ready_…`) directly assert the
behavioural contract that fix 1+2 implement.

Then the full gate:

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

All three must pass with zero warnings and zero diff.

---

## 2. Symptom S1 — multiple candidates with shared prefix

Start aish (use the same terminal that bash is in, on Linux/macOS):

```bash
cargo run --bin aish
```

Inside the aish prompt:

```text
aish> cd /tmp/aish-tab-test
```

### Test 2.1 — two candidates sharing a prefix

```text
aish> cd Doc<TAB>
```

- **Before fix**: nothing happens, or the prompt flickers, no candidate list.
- **After fix**: a list of candidates is shown:
  ```
  Documents/  Downloads/
  ```
  Press `<TAB>` again to highlight the first; press `<TAB>` again to move; press `<Enter>` to select.

### Test 2.2 — narrow to unique

```text
aish> cd Docu<TAB>
```

- **After fix**: line is auto-extended to `cd Documents/`.

### Test 2.3 — partial path

```text
aish> cd ../Doc<TAB>
```

- **After fix**: list shows `../Documents/  ../Downloads/`.

---

## 3. Symptom S2 — screen blanking

### Test 3.1 — repeated Tab

```text
aish> gi<TAB>
aish> gi<TAB><TAB>
aish> ls /ho<TAB>
aish> git<TAB>
```

- **Before fix**: occasional blank screen, stray escape codes, prompt cursor in wrong place.
- **After fix**: every Tab resolves cleanly, the prompt is redrawn correctly.

### Test 3.2 — mixed keystrokes

```text
aish> cd Doc<TAB>u<TAB>
aish> ls /<TAB>hom<TAB>
aish> git sta<TAB>
```

- No "screen disappeared" symptom.

---

## 4. Symptom S3 — stale candidate list

### Test 4.1 — narrow to unique mid-path

```text
aish> cd /tmp/aish-tab-test/D<TAB>
```

Expected: candidates `Documents/  Downloads/`.

```text
aish> cd /tmp/aish-tab-test/Docu<TAB>
```

- **Before fix**: still shows two candidates, or no completion.
- **After fix**: completes to `cd /tmp/aish-tab-test/Documents/`.

### Test 4.2 — narrow one key at a time

```text
aish> cd /tmp/aish-tab-test/D<TAB>o<TAB>c<TAB>u<TAB>
```

- Each Tab reduces to one unique candidate.

---

## 5. Fix 3 — no spurious `./` prefix

### Test 5.1 — bare filename

```text
aish> cd Doc<TAB>
```

After the line is auto-extended, look at the line:

- **Before fix**: `cd ./Documents/`
- **After fix**: `cd Documents/`

### Test 5.2 — full path

```text
aish> cd /tmp/aish-tab-test/Doc<TAB>
```

- **After fix**: `cd /tmp/aish-tab-test/Documents/` (no `./` anywhere).

### Test 5.3 — paths with `/`

```text
aish> ls Docu/<TAB>
```

If `Documents/` contains a child, it should complete correctly without `./` interference.

---

## 6. Regression check (other features still work)

### Test 6.1 — normal commands

```text
aish> ls -la /tmp/aish-tab-test
aish> pwd
aish> cd ~
aish> cd /tmp
```

All execute normally. The strictness added to `take_submission` does not break user-command completion.

### Test 6.2 — AI mode

```text
aish> ;列出当前目录所有文件
```

AI responds normally (depends on having a working API key).

### Test 6.3 — common user inputs

```text
aish> ls<TAB>
aish> git status<TAB>
aish> cd /etc/host<TAB>
```

All resolve as expected.

---

## 7. PR metadata check

On GitHub, open the PR and verify:

- Title: `fix(pty): drain stale control-pipe events before tab completion query (#207)`
- Base: `main` ← head: `fix/207-tab-completion`
- Files: 4 changed (+91 / -5) — after this verification doc is added, 6 files
- Commit message contains `Fixes #207` (auto-closes the issue on merge)

---

## 8. Go / no-go checklist

| Category | Item | Pass? |
|---|---|---|
| Unit | `cargo test -p aish-pty --lib command_state` all pass | ☐ |
| Unit | New 2 regression tests exist and pass | ☐ |
| S1 | `cd Doc<TAB>` shows list | ☐ |
| S1 | `cd Docu<TAB>` auto-completes | ☐ |
| S2 | Multiple Tabs no blank screen | ☐ |
| S3 | Unique candidate not stale | ☐ |
| Fix 3 | No `./` prefix | ☐ |
| Regression | User commands unaffected | ☐ |
| Regression | AI mode unaffected | ☐ |
| Integration | `cargo test --workspace` passes | ☐ |
| Integration | `cargo clippy -D warnings` passes | ☐ |
| Integration | `cargo fmt --check` passes | ☐ |
| Integration | `cargo build --release` passes | ☐ |
| PR | Branch / title / description correct | ☐ |

---

## 9. Notes

1. **PTY is a Unix thing**: Symptoms S1/S2/S3 only reproduce on Linux/macOS
   terminals. Windows `cmd` and `PowerShell` do not use PTY, so the bug is
   invisible there.
2. **WSL is supported**: the project officially targets Linux, but Windows
   users with WSL can run all of the above steps. The WSL Ubuntu distro is
   the recommended verification environment.
3. **No real network needed** for the tab-completion fix; you only need a
   network connection if you also exercise the AI features.
4. **To roll back and compare**: `git checkout main` puts you on the
   pre-fix commit; the same Tab presses will then reproduce the bug.
5. **bash 4.2 compatibility**: a previous fix
   ([`34a1b83`](https://github.com/AI-Shell-Team/aish/commit/34a1b83)) made
   completion work on CentOS 7 bash 4.2. PR #207 stacks on top of that and
   is independent.

---

## 10. What if a check fails?

If a regression test fails:

1. Make sure you are on the PR commit (`git log -1` shows `37ff508`).
2. Re-run with a clean target: `cargo clean -p aish-pty && cargo test -p aish-pty --lib`.
3. Check `crates/aish-pty/src/command_state.rs` for `take_submission` —
   the strict-`None` rule is on lines 184-197 (or near the end of the
   function in the PR's version).
4. File a comment on the PR with the failing test name and full output.
