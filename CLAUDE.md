# bunfiiliate

Open-source affiliate tracking + commission engine in Rust. Plan: `docs/PLAN.md`.

## Directives

1. **Caveman mode** in chat. Terse, no filler.
2. **Read `brain/index.md` first.** Navigate index-first via `[[links]]` and `/path/to/concept.md`.
3. **Update the brain** after each task: task status in `brain/tasks/index.md`, new entry at top of `brain/log.md`, new lessons in `brain/wisdom/`.
4. **Validate** the bundle: `python ~/.claude/skills/okf/scripts/validate.py brain --strict`.
5. **Git flow**: `prod` <- `staging` <- `feat/*`. Merge `--no-ff` only. Never commit on prod/staging. Short messages: `feat: add x`.
6. **Clean-room**: copy nothing from closed-source projects.
7. **No push** until the local final gate (`make ci`, `ocr`, `e2e`) passes.

## Commands

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
make ci   # fmt, clippy, test, deny (after T00)
```
