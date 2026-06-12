# 手动验证手册 — PR #207（Tab 补全回归）

> 配套：https://github.com/AI-Shell-Team/aish/pull/207
>
> 本手册说明如何在 Linux/macOS 终端上**手动**验证 issue
> [#207](https://github.com/AI-Shell-Team/aish/issues/207)（v0.3.0 Tab 补全回归）的修复。

## 本 PR 修复的内容

| # | 症状 | 根因 | 文件 |
|---|---|---|---|
| 1 | 多个候选共享前缀不补全 | `query_completions` 发新 `__aish_complete` 请求前没排空控制管道；`forward_readline_tab` 的 `set -o emacs` / `set +o emacs; set +o vi` 探测留下的 `PromptReady{command_seq:null}` 事件被新 submission 误匹配，wait 循环提前退出。 | `crates/aish-pty/src/aish_completion.rs` |
| 2 | Tab 后屏幕空白 | 同上 + readline-tab 探测残留的 PTY echo 状态。 | (由修复 1 覆盖) |
| 3 | 只剩唯一候选仍报两个 | `PromptReady{null}` 能抢走带 seq 的 backend submission。`CommandState::take_submission(None)` 无条件拿走 `active_submission`。 | `crates/aish-pty/src/command_state.rs` |
| 4 | `cd Doc<TAB>` 变成 `cd ./Documents/`（多余的 `./`） | bash 的 `_aish_resolve_compreply` 给裸文件名补全加了 `./` 前缀。 | `crates/aish-pty/src/bash_rc_wrapper.sh` |

## 0. 准备

### 0.1 切到 PR commit

```bash
git clone https://github.com/AI-Shell-Team/aish.git   # 或你自己的 fork
cd aish
git fetch origin pull/207/head:pr-207
git checkout pr-207

# 确认在 PR commit 上
git log -1 --oneline
# 预期: 37ff508 fix(pty): drain stale control-pipe events before tab completion query (#207)
```

### 0.2 编译

```bash
rustup toolchain install 1.95.0 --profile minimal --component clippy --component rustfmt
cargo build --release
```

或直接 `cargo run --bin aish`（debug 模式跑）。

### 0.3 准备测试目录

```bash
mkdir -p /tmp/aish-tab-test/Documents /tmp/aish-tab-test/Downloads
ls /tmp/aish-tab-test
# 预期: Documents  Downloads
```

### 0.4 LLM API key（仅在测 AI 功能时需要）

第一次启动 aish 需要 model + api_key。如果只想测补全，可以设 dummy key 在 `~/.config/aish/config.yaml`：

```yaml
model: openai/gpt-4o-mini
api_base: https://api.openai.com/v1
api_key: dummy
```

补全功能不依赖真 key（AI 命令 `;xxx` 会失败，但 Tab 补全仍正常）。

---

## 1. 自动化检查（先跑这些——最快、最确定）

```bash
cargo test -p aish-pty --lib command_state
```

预期输出（9 个测试全过，含 2 个新增回归）：

```
test command_state::tests::test_stale_prompt_ready_with_null_seq_does_not_consume_backend_submission ... ok
test command_state::tests::test_prompt_ready_with_null_seq_still_matches_user_submission ... ok
test command_state::tests::test_register_and_complete_user_command ... ok
... (6 more)
test result: ok. 9 passed; 0 failed
```

两个新测试（`test_stale_prompt_ready_…`）**直接断言** 修复 1+2 实现的语义。

然后跑完整门禁：

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

三项必须全部通过：0 警告、0 diff。

---

## 2. 症状 S1：多候选共享前缀不补全

启动 aish（在 Linux/macOS 真实终端，不要在 Windows cmd）：

```bash
cargo run --bin aish
```

在 aish 提示符下：

```text
aish> cd /tmp/aish-tab-test
```

### 测试 2.1：两个候选共享前缀

```text
aish> cd Doc<TAB>
```

- **修复前**：无反应 / 提示符闪烁 / 没候选列表
- **修复后**：弹出候选列表
  ```
  Documents/  Downloads/
  ```
  再按 `<TAB>` 高亮第一个；再 `<TAB>` 移动；`<Enter>` 选中

### 测试 2.2：缩窄到唯一

```text
aish> cd Docu<TAB>
```

- **修复后**：自动补全为 `cd Documents/`

### 测试 2.3：部分路径

```text
aish> cd ../Doc<TAB>
```

- **修复后**：列表显示 `../Documents/  ../Downloads/`

---

## 3. 症状 S2：屏幕空白

### 测试 3.1：连续 Tab

```text
aish> gi<TAB>
aish> gi<TAB><TAB>
aish> ls /ho<TAB>
aish> git<TAB>
```

- **修复前**：偶尔屏幕空白、残留 ANSI 转义、提示符光标错位
- **修复后**：每次 Tab 干净解决，提示符正确重绘

### 测试 3.2：混合按键

```text
aish> cd Doc<TAB>u<TAB>
aish> ls /<TAB>hom<TAB>
aish> git sta<TAB>
```

- 没有"屏幕突然消失"现象

---

## 4. 症状 S3：残留候选

### 测试 4.1：缩窄到唯一

```text
aish> cd /tmp/aish-tab-test/D<TAB>
```

预期：候选 `Documents/  Downloads/`

```text
aish> cd /tmp/aish-tab-test/Docu<TAB>
```

- **修复前**：仍显示两个候选 / 或不补全
- **修复后**：补全为 `cd /tmp/aish-tab-test/Documents/`

### 测试 4.2：每次按键缩窄

```text
aish> cd /tmp/aish-tab-test/D<TAB>o<TAB>c<TAB>u<TAB>
```

- 每次 Tab 缩窄到唯一候选

---

## 5. 修复 3：无 `./` 前缀

### 测试 5.1：裸文件名

```text
aish> cd Doc<TAB>
```

补全后看命令行内容：

- **修复前**：`cd ./Documents/`
- **修复后**：`cd Documents/`

### 测试 5.2：完整路径

```text
aish> cd /tmp/aish-tab-test/Doc<TAB>
```

- **修复后**：`cd /tmp/aish-tab-test/Documents/`（任何位置都没有 `./`）

### 测试 5.3：含 `/` 的 token

```text
aish> ls Docu/<TAB>
```

如果 `Documents/` 下有子目录，应正常补全，不受 `./` 干扰。

---

## 6. 回归检查（其他功能没坏）

### 测试 6.1：普通命令

```text
aish> ls -la /tmp/aish-tab-test
aish> pwd
aish> cd ~
aish> cd /tmp
```

都正常执行。`take_submission` 严格化不破坏用户命令补全。

### 测试 6.2：AI 模式

```text
aish> ;列出当前目录所有文件
```

AI 正常响应（依赖真 API key）。

### 测试 6.3：常用用户输入

```text
aish> ls<TAB>
aish> git status<TAB>
aish> cd /etc/host<TAB>
```

都正常解析。

---

## 7. PR 元数据

GitHub 上打开 PR 检查：

- 标题：`fix(pty): drain stale control-pipe events before tab completion query (#207)`
- Base: `main` ← head: `fix/207-tab-completion`
- 文件：4 改（+91/-5）—— 加本验证文档后是 6 文件
- Commit 包含 `Fixes #207`（合入后自动关 issue）

---

## 8. Go / no-go 清单

| 类别 | 项 | 通过？ |
|---|---|---|
| 单元 | `cargo test -p aish-pty --lib command_state` 全过 | ☐ |
| 单元 | 新增 2 个回归测试存在并通过 | ☐ |
| S1 | `cd Doc<TAB>` 弹列表 | ☐ |
| S1 | `cd Docu<TAB>` 自动补全 | ☐ |
| S2 | 多次 Tab 无白屏 | ☐ |
| S3 | 唯一候选不残留 | ☐ |
| 修复 3 | 无 `./` 前缀 | ☐ |
| 回归 | 用户命令不受影响 | ☐ |
| 回归 | AI 模式不受影响 | ☐ |
| 集成 | `cargo test --workspace` 通过 | ☐ |
| 集成 | `cargo clippy -D warnings` 通过 | ☐ |
| 集成 | `cargo fmt --check` 通过 | ☐ |
| 集成 | `cargo build --release` 通过 | ☐ |
| PR | 分支/标题/描述正确 | ☐ |

---

## 9. 注意事项

1. **PTY 是 Unix 概念**：症状 S1/S2/S3 只能在 Linux/macOS 终端复现。Windows `cmd` / `PowerShell` 不走 PTY，bug 不可见
2. **WSL 支持**：项目官方目标是 Linux，但 WSL Ubuntu 上可以跑完上述所有步骤
3. **无需网络**：tab 补全修复不依赖网络连接；只有 AI 功能才需要
4. **回滚对比**：`git checkout main` 切到修复前，同一个 Tab 按键能复现 bug
5. **bash 4.2 兼容**：之前的 [`34a1b83`](https://github.com/AI-Shell-Team/aish/commit/34a1b83) 修复让补全在 CentOS 7 bash 4.2 上工作。PR #207 叠加其上，独立

---

## 10. 出问题怎么办？

如果回归测试失败：

1. 确认在 PR commit 上（`git log -1` 显示 `37ff508`）
2. 清理 target 重跑：`cargo clean -p aish-pty && cargo test -p aish-pty --lib`
3. 检查 `crates/aish-pty/src/command_state.rs` 里的 `take_submission`——严格 `None` 规则在函数末尾
4. 在 PR 上贴失败的测试名和完整输出
